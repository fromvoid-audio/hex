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
fn terminal_main_limiter_keeps_two_resident_tracks_aligned_until_stop() {
    const CUT: usize = 512;
    const FADE: usize = 240;
    const FRAMES: usize = 2_048;
    let sources: [[Frame; FRAMES]; 2] = [
        std::array::from_fn(|frame| [0.0625, (frame % 79) as f32 / 2_048.0]),
        std::array::from_fn(|frame| [-((frame % 113) as f32) / 2_048.0, 0.03125]),
    ];
    let sum = |frame: usize| {
        let [first, second] = &sources;
        [
            first[frame][0] + second[frame][0],
            first[frame][1] + second[frame][1],
        ]
    };
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let mut media = BTreeMap::new();
    for (index, pcm) in sources.iter().enumerate() {
        let Created::Imported { asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: format!("dry input {index}"),
                path: format!("memory/dry-{index}.wav").into(),
                source_sample_rate: RATE,
                source_frames: FRAMES as u64,
                playback_frames: FRAMES as u64,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("resident dry track");
        };
        media.insert(asset, hex_engine::Audio::new(RATE, pcm.to_vec()).unwrap());
    }
    let table = Arc::new(hex_engine::SineTable::new());
    let prepare = |project: &Project| {
        Arc::new(
            hex_engine::PreparedRevision::prepare(project, &media, Arc::clone(&table), None)
                .unwrap(),
        )
    };
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("default Main limiter");
    };
    let before = prepare(editor.project());
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let after = prepare(editor.project());
    assert_eq!(before.arrangement().main_latency_frames(), DELAY as u32);
    assert_eq!(after.arrangement().main_latency_frames(), 0);

    for partition in [1, BLOCK] {
        let (mut control, mut callback, predecessor, receipt) =
            begin_insert_with_pending_track(Arc::clone(&before), Arc::clone(&after), None, CUT);
        let mut fade = [[0.0; 2]; FADE];
        for (chunk_index, chunk) in fade.chunks_mut(partition).enumerate() {
            render(&mut callback, chunk);
            for (offset, sample) in chunk.iter().enumerate() {
                let expected = sum(CUT + chunk_index * partition + offset - DELAY);
                assert!(
                    sample
                        .iter()
                        .zip(expected)
                        .all(|(actual, dry)| (*actual - dry).abs() < 1e-6),
                    "Main summing PCM during fade"
                );
            }
            assert_eq!(control.status.recording_source(), Some(predecessor));
            assert_eq!(receipt.state(), PublicationReceiptState::Pending);
        }
        let mut installed = [[0.0; 2]; BLOCK + 1];
        for chunk in installed.chunks_mut(partition) {
            render(&mut callback, chunk);
        }
        assert_eq!(receipt.state(), PublicationReceiptState::Installed);
        let active = control.status.recording_source().unwrap();
        assert_eq!(active.audio_revision, receipt.revision());
        assert_eq!(active.output_identity, predecessor.output_identity);
        assert_eq!(control.reclaim(), 1, "retired Main DSP returns to control");
        assert_eq!(
            installed,
            std::array::from_fn(|offset| sum(CUT + FADE + offset - DELAY))
        );

        control.stop();
        render(&mut callback, &mut installed[..1]);
        control.command(PlaybackCommand::Play).unwrap();
        render(&mut callback, &mut installed[..partition]);
        for (offset, &actual) in installed[..partition].iter().enumerate() {
            assert_eq!(actual, sum(offset));
        }
    }

    let [_, sibling] = editor.project().tracks() else {
        panic!("two dry tracks");
    };
    editor
        .execute(Command::Gain {
            track: sibling.id(),
            gain: 0.5,
        })
        .unwrap();
    let (control, mut callback, predecessor, receipt) =
        begin_insert_with_pending_track(before, prepare(editor.project()), None, CUT);
    let mut old = [[0.0; 2]; 64];
    render(&mut callback, &mut old);
    assert_eq!(
        receipt.state(),
        PublicationReceiptState::Refused(PlaybackHandoffRejection::DeviceInsertNeedsConfirmation)
    );
    assert_eq!(control.status.recording_source(), Some(predecessor));
    assert_eq!(old, std::array::from_fn(|offset| sum(CUT + offset - DELAY)));
}

#[test]
fn longer_main_limiter_removal_retains_pass_delay_until_stop() {
    let limiter = &hex_devices::limiter::DEFINITION;
    let state = limiter
        .patch_parameter(
            &limiter.default_state().unwrap(),
            hex_devices::limiter::parameter::LOOKAHEAD,
            ParameterValue::Choice(1),
        )
        .unwrap();
    terminal_inline_stage(state, RATE, 144);
}

#[test]
fn multi_track_multi_device_main_and_return_latency_floor_retains_until_stop() {
    const CUT: usize = 512;
    const FADE: usize = 240;
    const FRAMES: usize = 4_096;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let mut media = BTreeMap::new();
    let mut tracks = Vec::new();
    for index in 0..4 {
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: format!("track {index}"),
                path: format!("memory/multi-{index}.wav").into(),
                source_sample_rate: RATE,
                source_frames: FRAMES as u64,
                playback_frames: FRAMES as u64,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("track");
        };
        tracks.push(track);
        let level = 0.05 * (index + 1) as f32;
        media.insert(
            asset,
            hex_engine::Audio::new(RATE, vec![[level, -level]; FRAMES]).unwrap(),
        );
    }
    let Created::ReturnTrack(ret) = editor
        .execute(Command::AddReturnTrack { name: "bus".into() })
        .unwrap()
    else {
        panic!("return");
    };
    editor
        .execute(Command::SetReturnSend {
            track: tracks[0],
            return_track: ret,
            gain: 0.25,
        })
        .unwrap();
    editor
        .execute(Command::InsertDevice {
            track: tracks[1],
            before: None,
            state: hex_devices::utility::UTILITY_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap();
    // Multi-device Main chain: EQ8 + 3.0 ms Limiter (144 frames).
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: hex_devices::eq8::EQ8_DEFINITION.default_state().unwrap(),
        })
        .unwrap();
    let limiter = &hex_devices::limiter::DEFINITION;
    let limiter_3ms = limiter
        .patch_parameter(
            &limiter.default_state().unwrap(),
            hex_devices::limiter::parameter::LOOKAHEAD,
            ParameterValue::Choice(1),
        )
        .unwrap();
    let Created::Device(main_limiter) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: limiter_3ms,
        })
        .unwrap()
    else {
        panic!("main limiter");
    };
    let table = Arc::new(hex_engine::SineTable::new());
    let prepare = |project: &Project| {
        Arc::new(
            hex_engine::PreparedRevision::prepare(project, &media, Arc::clone(&table), None)
                .unwrap(),
        )
    };
    let before = prepare(editor.project());
    assert_eq!(before.arrangement().main_latency_frames(), 144);
    editor.execute(Command::RemoveDevice(main_limiter)).unwrap();
    let after_remove = prepare(editor.project());
    assert_eq!(after_remove.arrangement().main_latency_frames(), 0);

    // 1. Removing the Main limiter in a 4-track + Return + multi-device Main topology
    //    succeeds continuously with zero callback allocations, retaining 144 frames until Stop.
    let (mut control, mut callback, predecessor, receipt) =
        begin_insert_with_pending_track(Arc::clone(&before), Arc::clone(&after_remove), None, CUT);
    let mut fade = [[0.0; 2]; FADE];
    render(&mut callback, &mut fade);
    let mut after_cut = [[0.0; 2]; BLOCK];
    render(&mut callback, &mut after_cut);
    assert_eq!(receipt.state(), PublicationReceiptState::Installed);
    let latency = |callback: &Callback| {
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::current)
            .and_then(|active| active.playback.main_output_latency_frames())
    };
    assert_eq!(latency(&callback), Some(144));
    assert_eq!(control.reclaim(), 1);

    // 2. Inserting a 1.5 ms (72-frame) Limiter fits inside the active 144-frame pass floor
    //    and installs continuously while keeping total output latency pinned at 144 frames.
    let Created::Device(_) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: limiter.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("1.5ms limiter");
    };
    let after_reinsert = prepare(editor.project());
    assert_eq!(after_reinsert.arrangement().main_latency_frames(), 72);
    let current_sub = control.status.recording_source().unwrap();
    assert_eq!(current_sub.output_identity, predecessor.output_identity);
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let candidate = Playback::try_from_prepared_with_session(after_reinsert).unwrap();
    let revision = control
        .publish_unaccounted_pcm_obligation(
            candidate,
            PublicationPolicy::DeviceInsert {
                predecessor: current_sub,
                fence,
            },
            5,
            true,
            false,
            Some(hex_engine::AudioSourceContext {
                project_generation: 5,
                editor_revision: 9,
            }),
        )
        .unwrap();
    let reinsert_receipt = control.fresh_runtime_receipt(revision).unwrap();
    render(&mut callback, &mut after_cut);
    assert_eq!(reinsert_receipt.state(), PublicationReceiptState::Installed);
    assert_eq!(latency(&callback), Some(144));

    // 3. Stopping clears the retained pass-level delay so the next Play runs at natural 72 frames.
    control.stop();
    render(&mut callback, &mut after_cut[..1]);
    assert_eq!(latency(&callback), Some(72));
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

        const SEEK: u64 = 1_024;
        // A running seek fades out at the old cursor before clearing the ring,
        // then fades in at the new cursor (128 frames at 48 kHz).
        let seek_fade = (128 * sample_rate / RATE) as usize;
        let mut after_seek = vec![[0.0; 2]; 2 * seek_fade + latency_frames + 64];
        control.command(PlaybackCommand::Seek(SEEK)).unwrap();
        for chunk in after_seek.chunks_mut(partition) {
            render(&mut callback, chunk);
        }
        let cleared = &after_seek[seek_fade..seek_fade + latency_frames];
        assert!(cleared.iter().all(|sample| *sample == [0.0; 2]));
        let audible = &after_seek[2 * seek_fade + latency_frames..];
        let first = SEEK as usize + seek_fade;
        assert_eq!(audible, &pcm[first..first + audible.len()]);

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
