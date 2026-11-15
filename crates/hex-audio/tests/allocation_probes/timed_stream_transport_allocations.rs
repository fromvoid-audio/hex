//! Borrowed page timed replay on the caller thread; no device or page worker.
use hex_engine::{
    BorrowedMediaPage, Clip, Frame, MediaPages, PlaybackCommand, Project as EngineProject,
    STREAM_PAGE_FRAMES, StreamSourceKey, StreamedSourceMetadata, TimedTransportCommand, Track,
    playback::Playback,
};
use hex_project::{Command, Created, Editor, Project};
use std::sync::Arc;

struct Pages {
    key: StreamSourceKey,
    pcm: Vec<Frame>,
    requests: usize,
}
impl MediaPages for Pages {
    fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
        if key != self.key {
            return None;
        }
        let start = usize::try_from(index)
            .ok()?
            .checked_mul(STREAM_PAGE_FRAMES)?;
        let end = start.checked_add(STREAM_PAGE_FRAMES)?.min(self.pcm.len());
        (start < end).then_some(BorrowedMediaPage {
            page_index: index,
            frames: &self.pcm[start..end],
        })
    }
    fn request_page(&mut self, _: StreamSourceKey, _: u64) {
        self.requests += 1;
    }
}

#[test]
fn timed_stream_replay_does_not_allocate_or_free() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: (STREAM_PAGE_FRAMES + 16) as u64,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let key = StreamSourceKey::new(7, asset);
    let meta = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        (STREAM_PAGE_FRAMES + 16) as u64,
        (STREAM_PAGE_FRAMES + 16) as u64,
        0,
        [0x5a; 32],
    )
    .unwrap();
    let clip = Clip::streamed_region(Arc::new(meta), 0, 12, 4093, 12, false).unwrap();
    let project =
        EngineProject::new(48_000, vec![Track::new(vec![clip], 1.0, false).unwrap()]).unwrap();
    let mut playback = Playback::new(project);
    let mut pages = Pages {
        key,
        pcm: vec![[0.25; 2]; STREAM_PAGE_FRAMES + 16],
        requests: 0,
    };
    let commands = [
        TimedTransportCommand {
            offset: 0,
            command: PlaybackCommand::Play,
        },
        TimedTransportCommand {
            offset: 4,
            command: PlaybackCommand::Seek(1),
        },
        TimedTransportCommand {
            offset: 8,
            command: PlaybackCommand::Stop,
        },
    ];
    let mut output = [[0.0; 2]; 12];
    let mut result = Ok(());
    let counts = allocation_counter::measure(|| {
        result = playback.render_with_timed_transport_and_pages(&mut output, &commands, &mut pages);
    });
    assert!(result.is_ok());
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(&output[..8], &[[0.25; 2]; 8]);
    assert_eq!(&output[8..], &[[0.0; 2]; 4]);
    assert_eq!(pages.requests, 0);
}

#[test]
fn timed_stream_signal_observation_does_not_allocate_or_free() {
    use hex_engine::{PreparedRevision, SineTable, compile::PreparedAudioSource};
    use std::collections::BTreeMap;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Imported { asset, track, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "stream".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 8192,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap(),
        })
        .unwrap();
    let key = StreamSourceKey::new(7, asset);
    let source =
        StreamedSourceMetadata::new(key, 48_000, 48_000, 8192, 8192, 0, [0x5a; 32]).unwrap();
    let media = BTreeMap::from([(asset, PreparedAudioSource::Streamed(Arc::new(source)))]);
    let revision = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            editor.project(),
            &media,
            7,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = Playback::from_prepared(revision);
    let mut pages = Pages {
        key,
        pcm: vec![[0.25; 2]; 8192],
        requests: 0,
    };
    let commands = [
        TimedTransportCommand {
            offset: 0,
            command: PlaybackCommand::PlayFrom(4090),
        },
        TimedTransportCommand {
            offset: 8,
            command: PlaybackCommand::Seek(2),
        },
    ];
    let mut output = [[0.0; 2]; 16];
    let mut node_calls = 0;
    let mut project_calls = 0;
    let mut timing_calls = 0;
    let mut node = |_, _, _, _: &[Frame]| node_calls += 1;
    let mut observe = |_, _, _: &[Frame]| project_calls += 1;
    let mut timing = |_, _, _, _: &hex_engine::PreparedTempoMap| timing_calls += 1;
    let tracks = [track.value()];
    let mut tap = hex_engine::ProjectOutputTap::new(&tracks, &mut observe)
        .unwrap()
        .with_timing(&mut timing);
    let mut result = Ok(());
    let counts = allocation_counter::measure(|| {
        result = playback.render_with_timed_transport_pages_and_signal_observers(
            &mut output,
            &commands,
            &mut pages,
            Some(&mut node),
            Some(&mut tap),
        );
    });
    assert!(result.is_ok());
    assert!(node_calls > 0 && project_calls > 0 && timing_calls > 0);
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(output, [[0.25; 2]; 16]);
    assert_eq!(pages.requests, 0);
}
