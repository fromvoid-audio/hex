//! A single-route reorder with an independent continuous Delay oracle.
use super::*;

#[test]
fn native_track_reorder_fades_only_its_track_and_carries_delay_repeats_once() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter::INVERT_LEFT};

    const FRAMES: usize = 4096;
    const CUT: usize = 2048;
    const FADE: usize = RATE.div_ceil(200) as usize;
    const SWITCH: usize = CUT + FADE;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let mut media = BTreeMap::new();
    let mut add_audio = |name: &str, pcm: Vec<Frame>| {
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: name.into(),
                path: format!("{name}.wav").into(),
                sample_rate: RATE,
                frames: FRAMES as u64,
            })
            .unwrap()
        else {
            panic!("audio track");
        };
        media.insert(asset, hex_engine::Audio::new(RATE, pcm).unwrap());
        (track, asset)
    };
    let mut input = vec![[0.125, -0.0625]; FRAMES];
    input[1700] = [0.5, -0.25];
    input[SWITCH + 64] = [0.375, -0.1875];
    let (track, asset) = add_audio("reordered", input.clone());
    let independent: Vec<Frame> = (0..FRAMES)
        .map(|index| [0.0625 + index as f32 / 131_072.0, 0.03125])
        .collect();
    let (unrelated, _) = add_audio("unrelated", independent.clone());
    let invert = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            INVERT_LEFT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let Created::Device(utility) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: invert,
        })
        .unwrap()
    else {
        panic!("utility");
    };
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: feedback_delay(20.0),
        })
        .unwrap();
    let table = Arc::new(hex_engine::SineTable::new());
    let prepare = |project: &Project, pcm: &BTreeMap<_, _>| {
        Arc::new(
            hex_engine::PreparedRevision::prepare(project, pcm, Arc::clone(&table), None).unwrap(),
        )
    };
    let before = prepare(editor.project(), &media);
    let mut reference_editor = Editor::new(editor.project().clone(), 8).unwrap();
    reference_editor
        .execute(Command::RemoveDevice(utility))
        .unwrap();
    reference_editor
        .execute(Command::RemoveTrack(unrelated))
        .unwrap();
    let mut shaped = input;
    for frame in &mut shaped[..SWITCH] {
        frame[0] = -frame[0];
    }
    let mut reference_media = media.clone();
    reference_media.insert(asset, hex_engine::Audio::new(RATE, shaped).unwrap());
    let mut oracle = Playback::try_from_prepared_with_session(prepare(
        reference_editor.project(),
        &reference_media,
    ))
    .unwrap();
    oracle.apply(PlaybackCommand::Play);
    let mut expected = vec![[0.0; 2]; FRAMES];
    oracle.render(&mut expected).unwrap();
    for (index, frame) in expected.iter_mut().enumerate() {
        if index >= SWITCH {
            frame[0] = -frame[0];
        }
        let gain = if index < CUT {
            1.0
        } else if index < SWITCH {
            (SWITCH - index - 1) as f32 / FADE as f32
        } else {
            ((index - SWITCH + 1) as f32 / FADE as f32).min(1.0)
        };
        for channel in 0..2 {
            frame[channel] = frame[channel] * gain + independent[index][channel];
        }
    }
    editor
        .execute(Command::ReorderDevices {
            owner: hex_project::DeviceChainOwner::Track(track),
            selected: hex_project::DeviceSelectionIds::new(&[utility]).unwrap(),
            before: None,
        })
        .unwrap();
    let after = prepare(editor.project(), &media);
    for stop in [false, true] {
        let (control, mut callback, source, receipt) =
            begin_insert_with_pending_track(Arc::clone(&before), Arc::clone(&after), None, CUT);
        render(&mut callback, &mut [[0.0; 2]; 32]);
        assert_eq!(receipt.state(), PublicationReceiptState::Pending);
        let mut resumed = [[0.0; 2]; FADE];
        if stop {
            control.stop();
            render(&mut callback, &mut resumed);
            assert_eq!(resumed, [[0.0; 2]; FADE]);
            assert_eq!(control.status.position.load(Acquire), 0);
            assert_eq!(
                receipt.state(),
                PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged)
            );
        } else {
            assert_eq!(receipt.cancel(), FreshRuntimeCancellationResult::Cancelled);
            render(&mut callback, &mut resumed);
            assert_eq!(receipt.state(), PublicationReceiptState::Cancelled);
            let mut reference =
                Playback::try_from_prepared_with_session(Arc::clone(&before)).unwrap();
            reference.apply(PlaybackCommand::Play);
            reference
                .render(&mut vec![[0.0; 2]; CUT + 32 + FADE - 1])
                .unwrap();
            let mut reference_pcm = [[0.0; 2]; 1];
            reference.render(&mut reference_pcm).unwrap();
            assert_eq!(resumed[FADE - 1], reference_pcm[0]);
            assert_eq!(control.status.recording_source(), Some(source));
        }
    }
    for partition in [1, BLOCK] {
        let (control, mut callback, source, receipt) =
            begin_insert_with_pending_track(Arc::clone(&before), Arc::clone(&after), None, CUT);
        let mut pcm = vec![[0.0; 2]; FRAMES - CUT];
        for chunk in pcm[..FADE].chunks_mut(partition) {
            render(&mut callback, chunk);
            assert_eq!(receipt.state(), PublicationReceiptState::Pending);
            assert_eq!(control.status.recording_source(), Some(source));
        }
        for chunk in pcm[FADE..].chunks_mut(partition) {
            render(&mut callback, chunk);
            assert_eq!(receipt.state(), PublicationReceiptState::Installed);
        }
        for (index, (actual, expected)) in pcm.iter().zip(&expected[CUT..]).enumerate() {
            assert!(
                (actual[0] - expected[0]).abs() < 1e-6 && (actual[1] - expected[1]).abs() < 1e-6,
                "partition {partition}, frame {index}: {actual:?} vs {expected:?}"
            );
        }
        assert_eq!(control.status.position.load(Acquire), FRAMES as u64);
    }
}

fn render(callback: &mut Callback, output: &mut [Frame]) {
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
