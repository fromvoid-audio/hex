//! One continuously running effect is the PCM oracle for a two-track move.
use super::*;
use hex_lattice_contract::NativeDeviceState;

#[derive(Clone, Copy)]
enum ReceivingTrack {
    WithClip,
    Empty,
    WithUtility,
}

#[test]
fn running_delay_moves_once_between_two_faded_tracks_without_callback_allocations() {
    assert_running_effect_move(feedback_delay(20.0), ReceivingTrack::WithClip);
}

#[test]
fn running_reverb_moves_its_decay_to_the_destination_track() {
    assert_running_effect_move(full_wet_reverb(), ReceivingTrack::WithClip);
}

#[test]
fn running_effects_keep_sounding_when_moved_to_an_empty_track() {
    for state in [feedback_delay(20.0), full_wet_reverb()] {
        assert_running_effect_move(state, ReceivingTrack::Empty);
    }
}

#[test]
fn running_effects_move_after_the_receiving_tracks_utility() {
    for state in [feedback_delay(20.0), full_wet_reverb()] {
        assert_running_effect_move(state, ReceivingTrack::WithUtility);
    }
}

fn full_wet_reverb() -> NativeDeviceState {
    use hex_devices::reverb::{REVERB_DEFINITION, parameter};

    REVERB_DEFINITION
        .patch_parameter(
            &REVERB_DEFINITION.default_state().unwrap(),
            parameter::DRY_WET,
            ParameterValue::Float(100.0),
        )
        .unwrap()
}

fn assert_running_effect_move(state: NativeDeviceState, receiving_track: ReceivingTrack) {
    const FRAMES: usize = 4096;
    const CUT: usize = 2048;
    const FADE: usize = 240;
    const DESTINATION_ONSET: usize = 2600;
    let mut source = vec![[0.0; 2]; FRAMES];
    source[512..1536].fill([0.25, -0.125]);
    source[1600] = [0.5, -0.25];
    source[3000] = [0.25, 0.125];
    let mut destination = vec![[0.0; 2]; FRAMES];
    if !matches!(receiving_track, ReceivingTrack::Empty) {
        destination[DESTINATION_ONSET..].fill([-0.125, 0.25]);
    }
    let unrelated = vec![[0.0625, -0.03125]; FRAMES];
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let mut media = BTreeMap::new();
    let mut tracks = Vec::new();
    let mut assets = Vec::new();
    for (index, pcm) in [source.clone(), destination.clone(), unrelated.clone()]
        .into_iter()
        .enumerate()
    {
        if index == 1 && matches!(receiving_track, ReceivingTrack::Empty) {
            let Created::Track(track) = editor
                .execute(Command::AddTrack {
                    name: "empty destination".into(),
                })
                .unwrap()
            else {
                panic!("empty track")
            };
            tracks.push(track);
            continue;
        }
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: format!("move {index}"),
                path: format!("{index}.wav").into(),
                sample_rate: RATE,
                frames: FRAMES as u64,
            })
            .unwrap()
        else {
            panic!("track")
        };
        media.insert(asset, hex_engine::Audio::new(RATE, pcm).unwrap());
        tracks.push(track);
        assets.push(asset);
    }
    if matches!(receiving_track, ReceivingTrack::WithUtility) {
        use hex_devices::utility::{UTILITY_DEFINITION, parameter::INVERT_LEFT};
        let utility = UTILITY_DEFINITION
            .patch_parameter(
                &UTILITY_DEFINITION.default_state().unwrap(),
                INVERT_LEFT,
                ParameterValue::Bool(true),
            )
            .unwrap();
        editor
            .execute(Command::InsertDevice {
                track: tracks[1],
                before: None,
                state: utility,
            })
            .unwrap();
        // Apply Utility's polarity offline to the reference input, leaving the
        // imported PCM untouched so incorrect chain order changes the result.
        for frame in &mut destination {
            frame[0] = -frame[0];
        }
    }
    editor
        .execute(Command::SetTrackGains {
            gains: vec![(tracks[0], 0.25), (tracks[1], 0.75)],
        })
        .unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track: tracks[0],
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("effect")
    };
    let table = Arc::new(hex_engine::SineTable::new());
    let prepare = |project: &Project, media: &BTreeMap<_, _>| {
        Arc::new(
            hex_engine::PreparedRevision::prepare(project, media, Arc::clone(&table), None)
                .unwrap(),
        )
    };
    let mut reference_editor = Editor::new(editor.project().clone(), 8).unwrap();
    for track in [tracks[1], tracks[2]] {
        reference_editor
            .execute(Command::RemoveTrack(track))
            .unwrap();
    }
    reference_editor
        .execute(Command::Gain {
            track: tracks[0],
            gain: 1.0,
        })
        .unwrap();
    let before = prepare(editor.project(), &media);
    editor
        .execute(Command::MoveTrackDevices {
            source: tracks[0],
            destination: tracks[1],
            selected: hex_project::DeviceSelectionIds::new(&[device]).unwrap(),
        })
        .unwrap();
    let after = prepare(editor.project(), &media);
    if matches!(receiving_track, ReceivingTrack::WithUtility) {
        let mut changed = Editor::new(editor.project().clone(), 4).unwrap();
        let utility = changed.project().track(tracks[1]).unwrap().devices()[0].id();
        changed
            .execute(Command::SetDeviceBypass {
                device: utility,
                bypassed: true,
            })
            .unwrap();
        let (control, mut callback, predecessor, receipt) = begin_insert_with_pending_track(
            Arc::clone(&before),
            prepare(changed.project(), &media),
            None,
            CUT,
        );
        render(&mut callback, &mut [[0.0; 2]; 64]);
        assert_eq!(control.status.recording_source(), Some(predecessor));
        assert!(matches!(
            receipt.state(),
            PublicationReceiptState::Refused(_)
        ));
    }
    for partition in [1, 257] {
        let (control, mut callback, predecessor, receipt) =
            begin_insert_with_pending_track(Arc::clone(&before), Arc::clone(&after), None, CUT);
        let mut actual = vec![[0.0; 2]; FRAMES - CUT];
        let mut installed = None;
        for (block, pcm) in actual.chunks_mut(partition).enumerate() {
            let start = CUT + block * partition;
            render(&mut callback, pcm);
            match receipt.state() {
                PublicationReceiptState::Installed => {
                    installed.get_or_insert(start);
                }
                PublicationReceiptState::Pending => {
                    assert_eq!(control.status.recording_source(), Some(predecessor))
                }
                state => panic!("move was refused: {state:?}"),
            }
        }
        let switch = installed.unwrap();
        assert!(switch >= CUT + FADE && switch < CUT + FADE + partition);
        let mut combined = source.clone();
        combined[switch..].copy_from_slice(&destination[switch..]);
        let mut reference_media = media.clone();
        reference_media.insert(assets[0], hex_engine::Audio::new(RATE, combined).unwrap());
        let mut oracle = Playback::try_from_prepared_with_session(prepare(
            reference_editor.project(),
            &reference_media,
        ))
        .unwrap();
        oracle.apply(PlaybackCommand::Play);
        let mut wet = vec![[0.0; 2]; FRAMES];
        oracle.render(&mut wet).unwrap();
        assert!(
            wet[switch + FADE..DESTINATION_ONSET]
                .iter()
                .flatten()
                .any(|sample| sample.abs() > 1e-5),
            "pre-cut history must still sound before the destination supplies input"
        );
        for (offset, frame) in actual.iter().enumerate() {
            let at = CUT + offset;
            let fade = if at < switch {
                1.0 - ((offset + 1).min(FADE) as f32 / FADE as f32)
            } else {
                ((at - switch + 1).min(FADE) as f32) / FADE as f32
            };
            for lane in 0..2 {
                let moved = if at < switch {
                    0.25 * wet[at][lane] + 0.75 * destination[at][lane]
                } else {
                    0.25 * source[at][lane] + 0.75 * wet[at][lane]
                };
                let expected = fade * moved + unrelated[at][lane];
                assert!(
                    (frame[lane] - expected).abs() < 2e-6,
                    "partition={partition} frame={at} lane={lane}: {} vs {expected}",
                    frame[lane]
                );
            }
        }
    }
    // Cancel and source-fence invalidation retain the exact donor. A cancelled
    // move must restore both mixes while the original effect keeps processing.
    let mut oracle =
        Playback::try_from_prepared_with_session(prepare(reference_editor.project(), &media))
            .unwrap();
    oracle.apply(PlaybackCommand::Play);
    let mut wet = vec![[0.0; 2]; FRAMES];
    oracle.render(&mut wet).unwrap();
    for action in 0..3 {
        let (mut control, mut callback, predecessor, receipt) =
            begin_insert_with_pending_track(Arc::clone(&before), Arc::clone(&after), None, CUT);
        let mut prefix = [[0.0; 2]; 64];
        render(&mut callback, &mut prefix);
        assert_eq!(receipt.state(), PublicationReceiptState::Pending);
        match action {
            0 => assert_eq!(receipt.cancel(), FreshRuntimeCancellationResult::Cancelled),
            1 => control.stop(),
            _ => {
                control.command(PlaybackCommand::Play).unwrap();
            }
        }
        let mut restored = vec![[0.0; 2]; FRAMES - CUT - prefix.len()];
        for block in restored.chunks_mut(257) {
            render(&mut callback, block);
        }
        assert_eq!(control.status.recording_source(), Some(predecessor));
        assert_eq!(
            receipt.state(),
            if action == 0 {
                PublicationReceiptState::Cancelled
            } else {
                PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged)
            }
        );
        if action == 1 {
            assert!(restored.iter().all(|frame| *frame == [0.0; 2]));
        } else if action == 0 {
            let start_gain = 1.0 - prefix.len() as f32 / FADE as f32;
            for (offset, frame) in restored.iter().enumerate() {
                let at = CUT + prefix.len() + offset;
                let gain =
                    start_gain + (1.0 - start_gain) * (offset + 1).min(FADE) as f32 / FADE as f32;
                for lane in 0..2 {
                    let expected = gain * (0.25 * wet[at][lane] + 0.75 * destination[at][lane])
                        + unrelated[at][lane];
                    assert!(
                        (frame[lane] - expected).abs() < 2e-6,
                        "cancel frame={at} lane={lane}: {} vs {expected}",
                        frame[lane]
                    );
                }
            }
        }
    }
}

fn render(callback: &mut Callback, pcm: &mut [Frame]) {
    let measured = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(pcm).unwrap();
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
