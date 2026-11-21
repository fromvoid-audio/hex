//! Independent donor and fresh receiver renders verify a two-track cut.
use super::*;
use hex_lattice_contract::NativeDeviceState;

#[derive(Clone, Copy)]
enum TrackLayout {
    WithClip,
    Empty,
    WithUtility,
    FromUtility,
}

#[test]
fn running_delay_cuts_to_fresh_dsp_without_callback_allocations() {
    assert_running_effect_move(feedback_delay(20.0), TrackLayout::WithClip);
}

#[test]
fn running_reverb_cuts_to_fresh_dsp() {
    assert_running_effect_move(full_wet_reverb(), TrackLayout::WithClip);
}

#[test]
fn moving_running_effects_to_an_empty_track_ends_the_donor_wet_signal() {
    for state in [feedback_delay(20.0), full_wet_reverb()] {
        assert_running_effect_move(state, TrackLayout::Empty);
    }
}

#[test]
fn running_effects_move_after_the_receiving_tracks_utility() {
    for state in [feedback_delay(20.0), full_wet_reverb()] {
        assert_running_effect_move(state, TrackLayout::WithUtility);
    }
}

#[test]
fn running_effects_move_away_from_the_donor_tracks_utility() {
    for state in [feedback_delay(20.0), full_wet_reverb()] {
        assert_running_effect_move(state, TrackLayout::FromUtility);
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

fn assert_running_effect_move(state: NativeDeviceState, layout: TrackLayout) {
    const FRAMES: usize = 4096;
    const CUT: usize = 2048;
    const FADE: usize = 240;
    const DESTINATION_ONSET: usize = 2600;
    let mut source = vec![[0.0; 2]; FRAMES];
    source[512..1536].fill([0.25, -0.125]);
    source[1600] = [0.5, -0.25];
    source[3000] = [0.25, 0.125];
    let mut destination = vec![[0.0; 2]; FRAMES];
    if !matches!(layout, TrackLayout::Empty) {
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
        if index == 1 && matches!(layout, TrackLayout::Empty) {
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
    let utility_track = match layout {
        TrackLayout::FromUtility => Some(0),
        TrackLayout::WithUtility => Some(1),
        _ => None,
    };
    let utility = utility_track.map(|index| {
        use hex_devices::utility::{UTILITY_DEFINITION, parameter::INVERT_LEFT};
        let utility = UTILITY_DEFINITION
            .patch_parameter(
                &UTILITY_DEFINITION.default_state().unwrap(),
                INVERT_LEFT,
                ParameterValue::Bool(true),
            )
            .unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertDevice {
                track: tracks[index],
                before: None,
                state: utility,
            })
            .unwrap()
        else {
            panic!("utility");
        };
        // Apply Utility's polarity offline to the reference input, leaving the
        // imported PCM untouched so incorrect chain order changes the result.
        for frame in [&mut source, &mut destination][index].iter_mut() {
            frame[0] = -frame[0];
        }
        device
    });
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
    if let Some(utility) = utility.filter(|_| matches!(layout, TrackLayout::FromUtility)) {
        reference_editor
            .execute(Command::RemoveDevice(utility))
            .unwrap();
    }
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
    let render_reference = |input: &[Frame]| {
        let mut reference_pcm = input.to_vec();
        reference_pcm.resize(FRAMES, [0.0; 2]);
        let mut reference_media = media.clone();
        reference_media.insert(
            assets[0],
            hex_engine::Audio::new(RATE, reference_pcm).unwrap(),
        );
        let mut oracle = Playback::try_from_prepared_with_session(prepare(
            reference_editor.project(),
            &reference_media,
        ))
        .unwrap();
        oracle.apply(PlaybackCommand::Play);
        let mut wet = vec![[0.0; 2]; input.len()];
        oracle.render(&mut wet).unwrap();
        wet
    };
    let donor_wet = render_reference(&source);
    let before = prepare(editor.project(), &media);
    editor
        .execute(Command::MoveTrackDevices {
            source: tracks[0],
            destination: tracks[1],
            selected: hex_project::DeviceSelectionIds::new(&[device]).unwrap(),
        })
        .unwrap();
    let after = prepare(editor.project(), &media);
    if let Some(utility) = utility {
        let mut changed = Editor::new(editor.project().clone(), 4).unwrap();
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
        let receiver_wet = render_reference(&destination[switch..]);
        assert!(
            donor_wet[switch + FADE..DESTINATION_ONSET]
                .iter()
                .flatten()
                .any(|sample| sample.abs() > 1e-5),
            "the donor has buffered wet signal to discard before the receiver gets input"
        );
        if !matches!(layout, TrackLayout::Empty) {
            assert!(
                receiver_wet[DESTINATION_ONSET - switch..]
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 1e-5),
                "a fresh effect with the authored settings must process later receiver input"
            );
        }
        for (offset, frame) in actual.iter().enumerate() {
            let at = CUT + offset;
            let fade = if at < switch {
                1.0 - ((offset + 1).min(FADE) as f32 / FADE as f32)
            } else {
                ((at - switch + 1).min(FADE) as f32) / FADE as f32
            };
            for lane in 0..2 {
                let moved = if at < switch {
                    0.25 * donor_wet[at][lane] + 0.75 * destination[at][lane]
                } else {
                    0.25 * source[at][lane] + 0.75 * receiver_wet[at - switch][lane]
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
                    let expected = gain
                        * (0.25 * donor_wet[at][lane] + 0.75 * destination[at][lane])
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
