//! A departing Return fades independently of the surviving Return and dry route.
use super::*;

#[test]
fn removing_a_flat_return_cuts_its_delay_after_its_own_fade() {
    const FRAMES: usize = 2_048;
    const CUT: usize = 600;
    const FADE: usize = RATE.div_ceil(200) as usize;
    let (mut editor, track, prepare) = resident_input(vec![[0.25, -0.125]; FRAMES]);
    let mut add_return = |name: &str| {
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack { name: name.into() })
            .unwrap()
        else {
            panic!("return")
        };
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
        bus
    };
    let removed = add_return("departing delay");
    add_return("surviving dry return");
    editor
        .execute(Command::InsertReturnDevice {
            return_track: removed,
            before: None,
            state: feedback_delay(10.0),
        })
        .unwrap();
    let before = prepare(editor.project());
    editor.execute(Command::RemoveReturnTrack(removed)).unwrap();
    let after = prepare(editor.project());
    let render_whole = |revision| {
        let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
        playback.apply(PlaybackCommand::Play);
        let mut output = vec![[0.0; 2]; FRAMES];
        playback.render(&mut output).unwrap();
        output
    };
    let old_pcm = render_whole(Arc::clone(&before));
    let new_pcm = render_whole(Arc::clone(&after));
    assert!((old_pcm[CUT + FADE][0] - new_pcm[CUT + FADE][0]).abs() > 0.05);
    for partition in [1, BLOCK] {
        for cancel in [false, true] {
            let (control, mut callback, original, receipt) =
                begin_insert_with_pending_track(Arc::clone(&before), Arc::clone(&after), None, CUT);
            let cancel_at = FADE / 2;
            let prefix = if cancel { cancel_at } else { FADE };
            let mut output = vec![[0.0; 2]; FADE + BLOCK];
            for frames in output[..prefix].chunks_mut(partition) {
                render(&mut callback, frames);
                assert_eq!(receipt.state(), PublicationReceiptState::Pending);
                assert_eq!(control.status.recording_source(), Some(original));
            }
            if cancel {
                assert_eq!(receipt.cancel(), FreshRuntimeCancellationResult::Cancelled);
            }
            for frames in output[prefix..].chunks_mut(partition) {
                render(&mut callback, frames);
            }
            let active = control.status.recording_source().unwrap();
            assert_eq!(active.output_identity, original.output_identity);
            assert_eq!(
                control.status.position.load(Acquire),
                (CUT + output.len()) as u64
            );
            if cancel {
                assert_eq!(receipt.state(), PublicationReceiptState::Cancelled);
                assert_eq!(active, original);
            } else {
                assert_eq!(receipt.state(), PublicationReceiptState::Installed);
                assert_eq!(active.audio_revision, receipt.revision());
            }
            for (offset, frame) in output.iter().enumerate() {
                let gain = if offset < prefix {
                    1.0 - (offset + 1) as f32 / FADE as f32
                } else if cancel {
                    0.5 + 0.5 * (offset - prefix + 1).min(FADE) as f32 / FADE as f32
                } else {
                    0.0
                };
                for lane in 0..2 {
                    let index = CUT + offset;
                    let expected =
                        new_pcm[index][lane] + (old_pcm[index][lane] - new_pcm[index][lane]) * gain;
                    assert!(
                        (frame[lane] - expected).abs() < 1e-6,
                        "partition={partition} cancel={cancel} frame={index} lane={lane}: {} vs {expected}",
                        frame[lane]
                    );
                }
            }
        }
    }
}

fn render(callback: &mut Callback, output: &mut [Frame]) {
    let measured = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(output).unwrap();
    });
    assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
    assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
}
