//! Resident graph-edit oracle with two dry feeds and a shared Return.
use super::*;

#[test]
fn removing_plain_arrangement_track_fades_the_feed_and_keeps_shared_return_history() {
    use hex_devices::delay::{
        DEFINITION, DRY_WET, FEEDBACK, FILTER, HIGH_QUALITY, LEFT_SYNC, TIME,
    };

    const FRAMES: usize = 2_048;
    const CUT: usize = 512;
    const FADE: usize = RATE.div_ceil(200) as usize;
    const PREFIX: usize = 1 + 31 + 257;
    let feeds = [[0.25, -0.125], [0.125, -0.25]];
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "delay".into(),
        })
        .unwrap()
    else {
        panic!("return");
    };
    let mut echo = DEFINITION.default_state().unwrap();
    for parameter in [LEFT_SYNC, FILTER, HIGH_QUALITY] {
        echo = DEFINITION
            .patch_parameter(&echo, parameter, ParameterValue::Bool(false))
            .unwrap();
    }
    for (parameter, value) in [(TIME, 10.0), (FEEDBACK, 0.5), (DRY_WET, 1.0)] {
        echo = DEFINITION
            .patch_parameter(&echo, parameter, ParameterValue::Float(value))
            .unwrap();
    }
    editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: echo,
        })
        .unwrap();
    let mut media = BTreeMap::new();
    let mut import = |name: &str, sample: Frame| {
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: name.into(),
                path: format!("{name}.wav").into(),
                sample_rate: RATE,
                frames: FRAMES as u64,
            })
            .unwrap()
        else {
            panic!("audio import");
        };
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
        media.insert(
            asset,
            hex_engine::Audio::new(RATE, vec![sample; FRAMES]).unwrap(),
        );
        (track, asset)
    };
    let (removed, asset) = import("removed", feeds[0]);
    import("kept", feeds[1]);
    let table = Arc::new(hex_engine::SineTable::new());
    let prepare = |project: &Project, media: &BTreeMap<_, _>| {
        Arc::new(
            hex_engine::PreparedRevision::prepare(project, media, Arc::clone(&table), None)
                .unwrap(),
        )
    };
    let before = prepare(editor.project(), &media);
    assert_eq!(before.arrangement().main_latency_frames(), 0);

    // Independently shape the source PCM. Both feeds share the Delay's
    // original history, including the removed track's pre-cut contribution.
    let mut filtered = vec![feeds[0]; FRAMES];
    for (position, frame) in filtered[CUT..].iter_mut().enumerate() {
        let gain = FADE.saturating_sub(position + 1) as f32 / FADE as f32;
        frame.iter_mut().for_each(|sample| *sample *= gain);
    }
    let mut reference_media = media.clone();
    reference_media.insert(asset, hex_engine::Audio::new(RATE, filtered).unwrap());
    let reference = prepare(editor.project(), &reference_media);
    editor.execute(Command::RemoveTrack(removed)).unwrap();
    let after = prepare(editor.project(), &media);
    let oracle = |revision| {
        let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
        playback.apply(PlaybackCommand::Play);
        let mut pcm = vec![[0.0; 2]; FRAMES];
        playback.render(&mut pcm).unwrap();
        pcm
    };
    let expected = oracle(reference);
    let without_old_feed = oracle(Arc::clone(&after));
    assert!(
        expected[CUT + PREFIX..CUT + PREFIX + 256]
            .iter()
            .zip(&without_old_feed[CUT + PREFIX..])
            .any(|(kept, fresh)| (kept[0] - fresh[0]).abs() > 0.01)
    );

    let (control, mut callback, source, receipt) =
        begin_insert_with_pending_track(before, after, None, CUT);
    let mut pcm = [[0.0; 2]; PREFIX + 256];
    let mut offset = 0;
    for count in [1, 31, 257, 256] {
        measured_render(&mut callback, &mut pcm[offset..offset + count]);
        offset += count;
        if offset <= PREFIX {
            assert_eq!(receipt.state(), PublicationReceiptState::Pending);
            assert_eq!(control.status.recording_source(), Some(source));
        }
    }
    assert_eq!(receipt.state(), PublicationReceiptState::Installed);
    let active = control.status.recording_source().unwrap();
    assert_eq!(active.audio_revision, receipt.revision());
    assert_eq!(active.output_identity, source.output_identity);
    assert_eq!(
        control.status.position.load(Acquire),
        (CUT + pcm.len()) as u64
    );
    for (frame, (actual, expected)) in pcm.iter().zip(&expected[CUT..]).enumerate() {
        assert!(
            (actual[0] - expected[0]).abs() < 1e-6 && (actual[1] - expected[1]).abs() < 1e-6,
            "frame {frame}: {actual:?} vs {expected:?}"
        );
    }
}

#[test]
fn an_aborted_track_removal_restores_its_running_fader() {
    const FADE: usize = RATE.div_ceil(200) as usize;
    let (mut editor, removed, prepare) = resident_input(vec![SIGNAL; 2048]);
    let old = prepare(editor.project());
    editor.execute(Command::RemoveTrack(removed)).unwrap();
    let next = prepare(editor.project());
    for stop in [false, true] {
        let (control, mut callback, source, receipt) =
            begin_insert_with_pending_track(Arc::clone(&old), Arc::clone(&next), None, 384);
        let mut prefix = [[0.0; 2]; 32];
        measured_render(&mut callback, &mut prefix);
        assert_eq!(receipt.state(), PublicationReceiptState::Pending);
        assert!(prefix[0][0] < SIGNAL[0] && prefix[31][0] < prefix[0][0]);
        let mut resumed = [[0.0; 2]; FADE];
        let expected = if stop {
            control.stop();
            measured_render(&mut callback, &mut resumed);
            assert_eq!(resumed, [[0.0; 2]; FADE]);
            assert_eq!(control.status.position.load(Acquire), 0);
            let mut control = control;
            control.command(PlaybackCommand::Play).unwrap();
            measured_render(&mut callback, &mut resumed);
            PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged)
        } else {
            assert_eq!(receipt.cancel(), FreshRuntimeCancellationResult::Cancelled);
            measured_render(&mut callback, &mut resumed);
            assert!(resumed[0][0] > prefix[31][0]);
            assert_eq!(control.status.recording_source(), Some(source));
            PublicationReceiptState::Cancelled
        };
        assert_eq!(resumed[FADE - 1], SIGNAL);
        assert_eq!(receipt.state(), expected);
    }
}

fn measured_render(callback: &mut Callback, output: &mut [Frame]) {
    let stats = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(output).unwrap();
    });
    assert_eq!(
        (
            stats.count_total,
            stats.bytes_total,
            stats.count_current,
            stats.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
