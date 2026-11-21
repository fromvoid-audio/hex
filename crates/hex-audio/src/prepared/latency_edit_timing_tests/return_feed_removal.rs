//! A fading Return stops adding to a surviving Return without clearing its history.
use super::*;
use hex_devices::utility::{UTILITY_DEFINITION, parameter::GAIN};
use hex_project::ReturnId;

const CUT: usize = 2048;
const FRAMES: usize = 4096;
const FADE: usize = RATE.div_ceil(200) as usize;
const SEND: f32 = 0.5;
const UTILITY_GAIN: f32 = 0.5;
const DEPARTING_GAIN: f32 = SEND * UTILITY_GAIN;

#[test]
fn removing_a_return_fades_its_feed_and_keeps_the_downstream_delay_history() {
    assert_faded_removal(false);
}

#[test]
fn removing_a_return_keeps_the_surviving_feed_and_its_downstream_audio() {
    assert_faded_removal(true);
}

fn assert_faded_removal(relay: bool) {
    let mut dry = vec![[0.015625, -0.03125]; FRAMES];
    dry[512..1536].fill([0.25, -0.125]);
    dry[CUT + 512..CUT + 1024].fill([-0.25, 0.125]);
    let (mut editor, track, prepare) = resident_input(dry.clone());
    let departing = add_return(&mut editor, "departing return");
    let downstream = add_return(&mut editor, "downstream delay");
    let utility = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            GAIN,
            ParameterValue::Float(UTILITY_GAIN),
        )
        .unwrap();
    for (return_track, state) in [(departing, utility), (downstream, feedback_delay(17.0))] {
        editor
            .execute(Command::InsertReturnDevice {
                return_track,
                before: None,
                state,
            })
            .unwrap();
    }
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: departing,
            gain: SEND,
        })
        .unwrap();
    editor
        .execute(Command::SetReturnFeed {
            source: departing,
            destination: downstream,
            gain: SEND,
        })
        .unwrap();
    let surviving_edge = relay.then(|| {
        let destination = add_return(&mut editor, "surviving return");
        editor
            .execute(Command::SetReturnFeed {
                source: downstream,
                destination,
                gain: SEND,
            })
            .unwrap();
        editor
            .project()
            .authored_routing()
            .iter()
            .find(|feed| feed.destination() == destination)
            .unwrap()
            .id()
    });
    let old = prepare(editor.project());
    editor
        .execute(Command::RemoveReturnTrack(departing))
        .unwrap();
    assert!(
        editor
            .project()
            .authored_routing()
            .iter()
            .map(|feed| feed.id())
            .eq(surviving_edge)
    );
    let new = prepare(editor.project());
    assert_eq!(old.arrangement().main_latency_frames(), 0);
    assert_eq!(new.arrangement().main_latency_frames(), 0);

    for cancel in [false, true] {
        let prefix = if cancel { FADE / 2 } else { FADE };
        let expected = downstream_oracle(&dry, prefix, cancel, relay);
        if !cancel {
            let unfaded = render_whole(Arc::clone(&old), CUT);
            for (actual, expected) in unfaded.into_iter().zip(&expected[..CUT]) {
                assert!(
                    actual
                        .iter()
                        .zip(expected)
                        .all(|(a, b)| (a - b).abs() < 1e-6)
                );
            }
        }
        for partition in [1, BLOCK] {
            let (control, mut callback, original, receipt) =
                begin_insert_with_pending_track(Arc::clone(&old), Arc::clone(&new), None, CUT);
            let mut output = vec![[0.0; 2]; FRAMES - CUT];
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
            assert_eq!(control.status.position.load(Acquire), FRAMES as u64);
            if cancel {
                assert_eq!(receipt.state(), PublicationReceiptState::Cancelled);
                assert_eq!(active, original);
            } else {
                assert_eq!(receipt.state(), PublicationReceiptState::Installed);
                assert_eq!(active.audio_revision, receipt.revision());
            }
            for (offset, (sample, reference)) in output.iter().zip(&expected[CUT..]).enumerate() {
                assert!(
                    sample
                        .iter()
                        .zip(reference)
                        .all(|(a, b)| (a - b).abs() < 1e-6),
                    "partition={partition} cancel={cancel} frame={}: {sample:?} vs {reference:?}",
                    CUT + offset
                );
            }
        }
    }
}

fn add_return(editor: &mut Editor, name: &str) -> ReturnId {
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack { name: name.into() })
        .unwrap()
    else {
        panic!("return")
    };
    bus
}

fn downstream_oracle(dry: &[Frame], prefix: usize, cancel: bool, relay: bool) -> Vec<Frame> {
    // Drive an independent copy of the downstream Delay from a direct Track
    // send. The reference never routes a Return or publishes a replacement.
    let feed: Vec<_> = dry
        .iter()
        .enumerate()
        .map(|(at, frame)| {
            let gain = match at.checked_sub(CUT) {
                None => 1.0,
                Some(offset) if offset < prefix => 1.0 - (offset + 1) as f32 / FADE as f32,
                Some(offset) if cancel => {
                    0.5 + 0.5 * (offset - prefix + 1).min(FADE) as f32 / FADE as f32
                }
                Some(_) => 0.0,
            };
            frame.map(|sample| sample * gain)
        })
        .collect();
    let (mut editor, track, prepare) = resident_input(feed.clone());
    let bus = add_return(&mut editor, "independent delay");
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: DEPARTING_GAIN * SEND,
        })
        .unwrap();
    editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: feedback_delay(17.0),
        })
        .unwrap();
    let mut reference = render_whole(prepare(editor.project()), dry.len());
    if !cancel {
        let after = CUT + FADE..CUT + FADE + BLOCK;
        assert!(feed[after.clone()].iter().all(|frame| *frame == [0.0; 2]));
        assert!(reference[after].iter().any(|frame| frame[0].abs() > 1e-5));
    }
    for ((reference, feed), dry) in reference.iter_mut().zip(feed).zip(dry) {
        for lane in 0..2 {
            let wet = reference[lane] - feed[lane];
            reference[lane] = dry[lane] + feed[lane] * DEPARTING_GAIN + wet;
            if relay {
                // The surviving edge feeds an otherwise empty unity Return.
                reference[lane] += wet * SEND;
            }
        }
    }
    reference
}

fn render_whole(revision: Arc<hex_engine::PreparedRevision>, frames: usize) -> Vec<Frame> {
    let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
    playback.apply(PlaybackCommand::Play);
    let mut output = vec![[0.0; 2]; frames];
    playback.render(&mut output).unwrap();
    output
}

fn render(callback: &mut Callback, output: &mut [Frame]) {
    let measured = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(output).unwrap();
    });
    assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
    assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
}
