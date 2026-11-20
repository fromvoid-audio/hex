//! Withdrawn Main DSP and surviving aligned dry delay have different owners.
use super::*;

#[test]
fn terminal_overdrive_retires_while_its_pass_delay_survives_until_stop() {
    terminal_inline_stage(
        hex_devices::overdrive::OVERDRIVE_DEFINITION
            .default_state()
            .unwrap(),
        RATE,
        11,
    );
}

#[test]
fn terminal_saturator_retires_while_its_pass_delay_survives_until_stop() {
    terminal_inline_stage(
        hex_devices::saturator::SATURATOR_DEFINITION
            .default_state()
            .unwrap(),
        RATE,
        11,
    );
}

#[test]
fn terminal_rattle_retires_while_its_pass_delay_survives_until_stop() {
    terminal_inline_stage(
        hex_devices::rattle::RATTLE_DEFINITION
            .default_state()
            .unwrap(),
        RATE,
        11,
    );
}

#[test]
fn terminal_default_limiter_retires_while_its_pass_delay_survives_until_stop() {
    terminal_inline_stage(
        hex_devices::limiter::DEFINITION.default_state().unwrap(),
        RATE,
        72,
    );
}

#[test]
fn terminal_default_limiter_retains_288_frames_at_192_khz() {
    terminal_inline_stage(
        hex_devices::limiter::DEFINITION.default_state().unwrap(),
        192_000,
        288,
    );
}

#[test]
fn longer_main_limiter_removal_requires_consent() {
    let limiter = &hex_devices::limiter::DEFINITION;
    let state = limiter
        .patch_parameter(
            &limiter.default_state().unwrap(),
            hex_devices::limiter::parameter::LOOKAHEAD,
            ParameterValue::Choice(1),
        )
        .unwrap();
    let ([dry, delayed], _) = main_append_revisions(None, state, vec![SIGNAL; 2048]);
    assert_eq!(delayed.arrangement().main_latency_frames(), 144);
    let (control, mut callback, predecessor, receipt) =
        begin_insert_with_pending_track(delayed, dry, None, 512);
    render(&mut callback, &mut [[0.0; 2]; 257]);
    assert_eq!(
        receipt.state(),
        PublicationReceiptState::Refused(PlaybackHandoffRejection::DeviceInsertNeedsConfirmation)
    );
    assert_eq!(control.status.recording_source(), Some(predecessor));
}

fn terminal_inline_stage(
    state: hex_lattice_contract::NativeDeviceState,
    sample_rate: u32,
    latency_frames: usize,
) {
    const CUT: usize = 512;
    let fade_frames = sample_rate.div_ceil(200) as usize;
    let pcm: Vec<_> = (0..4096)
        .map(|i| {
            let value = 0.03125 + i as f32 / 131072.0;
            [value, -0.75 * value]
        })
        .collect();
    let (mut editor, _, prepare) = resident_input_at_rate(pcm.clone(), sample_rate);
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("terminal Main stage");
    };
    let before = prepare(editor.project());
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let after = prepare(editor.project());
    assert_eq!(
        before.arrangement().main_latency_frames(),
        latency_frames as u32
    );
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
        let mut fade = vec![[0.0; 2]; fade_frames];
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
        assert_eq!(latency(&callback), Some(latency_frames as u32));
        let start = CUT + fade_frames - latency_frames;
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
        assert_eq!(latency(&callback), Some(latency_frames as u32));
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
        let mut restored = vec![[0.0; 2]; fade_frames + 64];
        for chunk in restored.chunks_mut(partition) {
            render(&mut callback, chunk);
        }
        assert_eq!(receipt.state(), PublicationReceiptState::Cancelled);
        assert_eq!(control.status.recording_source(), Some(predecessor));
        let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&before)).unwrap();
        oracle.apply(PlaybackCommand::Play);
        let mut expected = vec![[0.0; 2]; CUT + 64 + restored.len()];
        oracle.render(&mut expected).unwrap();
        assert_eq!(restored[fade_frames..], expected[CUT + 64 + fade_frames..]);
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
