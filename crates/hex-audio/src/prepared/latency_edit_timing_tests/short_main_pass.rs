//! The withdrawn Main DSP and its surviving eleven-frame delay have different owners.
use super::*;

#[test]
fn terminal_overdrive_retires_while_its_pass_delay_survives_until_stop() {
    const CUT: usize = 512;
    const FADE: usize = 240;
    const LATENCY: usize = 11;
    let pcm: Vec<_> = (0..4096)
        .map(|i| {
            let value = 0.03125 + i as f32 / 131072.0;
            [value, -0.75 * value]
        })
        .collect();
    let (mut editor, _, prepare) = resident_input(pcm.clone());
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_devices::overdrive::OVERDRIVE_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("overdrive");
    };
    let before = prepare(editor.project());
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let after = prepare(editor.project());
    assert_eq!(before.arrangement().main_latency_frames(), LATENCY as u32);
    assert_eq!(after.arrangement().main_latency_frames(), 0);

    let latency = |callback: &Callback| {
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::current)
            .and_then(|active| active.playback.main_output_latency_frames())
    };
    for partition in [1, BLOCK] {
        let (mut control, mut callback, predecessor, receipt) =
            begin_insert_with_pending_track(Arc::clone(&before), Arc::clone(&after), None, CUT);
        let mut fade = [[0.0; 2]; FADE];
        for chunk in fade.chunks_mut(partition) {
            render(&mut callback, chunk);
            assert_eq!(receipt.state(), PublicationReceiptState::Pending);
            assert_eq!(control.status.recording_source(), Some(predecessor));
        }
        let mut actual = [[0.0; 2]; 512];
        for chunk in actual.chunks_mut(partition) {
            render(&mut callback, chunk);
        }
        let active = control.status.recording_source().unwrap();
        assert_eq!(receipt.state(), PublicationReceiptState::Installed);
        assert_eq!(active.audio_revision, receipt.revision());
        assert_eq!(active.output_identity, predecessor.output_identity);
        assert_eq!(latency(&callback), Some(LATENCY as u32));
        let start = CUT + FADE - LATENCY;
        assert_eq!(actual, pcm[start..start + actual.len()]);
        assert_eq!(
            control.reclaim(),
            1,
            "the old DSP graph must return to control"
        );

        let continued = control
            .publish(Playback::try_from_prepared_with_session(Arc::clone(&after)).unwrap())
            .unwrap();
        let mut next = [[0.0; 2]; 64];
        render(&mut callback, &mut next);
        assert_eq!(control.status.applied_revision.load(Acquire), continued);
        assert_eq!(
            next,
            pcm[start + actual.len()..start + actual.len() + next.len()]
        );

        control.command(PlaybackCommand::Pause).unwrap();
        render(&mut callback, &mut next);
        assert_eq!(latency(&callback), Some(LATENCY as u32));
        control.stop();
        render(&mut callback, &mut next);
        assert_eq!(latency(&callback), Some(0));
        control.command(PlaybackCommand::Play).unwrap();
        render(&mut callback, &mut next);
        assert_eq!(next, pcm[..next.len()]);

        let (control, mut callback, predecessor, receipt) =
            begin_insert_with_pending_track(Arc::clone(&before), Arc::clone(&after), None, CUT);
        render(&mut callback, &mut next);
        assert_eq!(receipt.state(), PublicationReceiptState::Pending);
        assert_eq!(receipt.cancel(), FreshRuntimeCancellationResult::Cancelled);
        let mut restored = [[0.0; 2]; FADE + 64];
        for chunk in restored.chunks_mut(partition) {
            render(&mut callback, chunk);
        }
        assert_eq!(receipt.state(), PublicationReceiptState::Cancelled);
        assert_eq!(control.status.recording_source(), Some(predecessor));
        let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&before)).unwrap();
        oracle.apply(PlaybackCommand::Play);
        let mut expected = vec![[0.0; 2]; CUT + 64 + restored.len()];
        oracle.render(&mut expected).unwrap();
        assert_eq!(restored[FADE..], expected[CUT + 64 + FADE..]);
    }
}

fn render(callback: &mut Callback, frames: &mut [Frame]) {
    let measured = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(frames).unwrap();
    });
    assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
    assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
}
