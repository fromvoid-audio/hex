//! A departing Return fades independently of the surviving Return and dry route.
use super::*;
use hex_devices::reverb::{REVERB_DEFINITION, parameter::DRY_WET};
use hex_lattice_contract::NativeDeviceState;

#[test]
fn removing_a_flat_return_cuts_its_delay_after_its_own_fade() {
    assert_flat_return_removal(
        [feedback_delay(10.0)],
        None,
        vec![[0.25, -0.125]; 2048],
        600,
        0.05,
    );
}

#[test]
fn removing_a_flat_return_cuts_its_reverb_after_its_own_fade() {
    assert_flat_return_removal(
        [full_wet_reverb()],
        Some(feedback_delay(17.0)),
        reverb_input(),
        2048,
        1e-5,
    );
}

#[test]
fn removing_a_flat_return_cuts_its_serial_delay_and_reverb_after_its_own_fade() {
    assert_flat_return_removal(
        [feedback_delay(10.0), full_wet_reverb()],
        Some(feedback_delay(17.0)),
        reverb_input(),
        2048,
        1e-5,
    );
}

fn full_wet_reverb() -> NativeDeviceState {
    REVERB_DEFINITION
        .patch_parameter(
            &REVERB_DEFINITION.default_state().unwrap(),
            DRY_WET,
            ParameterValue::Float(100.0),
        )
        .unwrap()
}

fn reverb_input() -> Vec<Frame> {
    let mut pcm = vec![[0.015625, -0.03125]; 4096];
    pcm[512..1536].fill([0.25, -0.125]);
    pcm
}

fn assert_flat_return_removal(
    departing: impl IntoIterator<Item = NativeDeviceState>,
    surviving: Option<NativeDeviceState>,
    dry_pcm: Vec<Frame>,
    cut: usize,
    minimum_wet_signal: f32,
) {
    const FADE: usize = RATE.div_ceil(200) as usize;
    let (mut editor, track, prepare) = resident_input(dry_pcm.clone());
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
    let removed = add_return("departing return");
    let survivor = add_return("surviving return");
    if let Some(state) = surviving {
        editor
            .execute(Command::InsertReturnDevice {
                return_track: survivor,
                before: None,
                state,
            })
            .unwrap();
    }
    for state in departing {
        editor
            .execute(Command::InsertReturnDevice {
                return_track: removed,
                before: None,
                state,
            })
            .unwrap();
    }
    let before = prepare(editor.project());
    editor.execute(Command::RemoveReturnTrack(removed)).unwrap();
    let after = prepare(editor.project());
    let render_whole = |revision| {
        let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
        playback.apply(PlaybackCommand::Play);
        let mut output = vec![[0.0; 2]; dry_pcm.len()];
        playback.render(&mut output).unwrap();
        output
    };
    let old_pcm = render_whole(Arc::clone(&before));
    let new_pcm = render_whole(Arc::clone(&after));
    let mut past_fade = cut + FADE..cut + FADE + BLOCK;
    assert!(
        past_fade
            .clone()
            .any(|at| (old_pcm[at][0] - new_pcm[at][0]).abs() > minimum_wet_signal),
        "the unedited Return must still sound past the cut"
    );
    assert!(
        past_fade.clone().all(|at| dry_pcm[at][0] != 0.0)
            && past_fade.any(|at| (new_pcm[at][0] - dry_pcm[at][0]).abs() > 1e-5),
        "the reference must contain both unaffected routes"
    );
    for partition in [1, BLOCK] {
        for cancel in [false, true] {
            let (control, mut callback, original, receipt) =
                begin_insert_with_pending_track(Arc::clone(&before), Arc::clone(&after), None, cut);
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
                (cut + output.len()) as u64
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
                    let index = cut + offset;
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
