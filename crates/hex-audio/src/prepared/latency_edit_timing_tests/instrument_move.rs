//! Direct Sine transfer must carry donor notes and admit only new receiver onsets.
use super::*;
use hex_project::{BeatTime, DeviceSelectionIds, TrackDeviceSlot};

#[test]
fn direct_sine_move_carries_the_donor_gate_and_never_chases_the_receiver() {
    const REQUEST: usize = 512;
    const FADE: usize = 240;
    const CUT: usize = REQUEST + FADE;
    const FRAMES: usize = 8_192;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Track(source) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "source".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("source instrument");
    };
    let device = editor
        .project()
        .track(source)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let Created::Track(destination) = editor
        .execute(Command::AddMidiTrack {
            name: "destination".into(),
        })
        .unwrap()
    else {
        panic!("destination track");
    };
    let mut place = |track| {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip");
        };
        clip
    };
    let donor_clip = place(source);
    let receiver_clip = place(destination);
    let make_note = |project: &Project, clip, frame: usize, frames: usize, pitch| {
        let tempo = project.tempo_map();
        Command::AddMidiNote {
            clip,
            start: tempo.frame_to_beat(frame as u64).unwrap(),
            duration: tempo.frame_to_beat(frames as u64).unwrap(),
            pitch,
            channel: 0,
            velocity: 100,
        }
    };
    let donor_note = make_note(editor.project(), donor_clip, 0, 1300, 59);
    let exact_onset = make_note(editor.project(), receiver_clip, CUT, 516, 69);
    let future_onset = make_note(editor.project(), receiver_clip, 2200, 728, 76);
    editor.execute(donor_note).unwrap();
    editor.execute(exact_onset).unwrap();
    editor.execute(future_onset).unwrap();
    editor
        .execute(Command::SetTrackGains {
            gains: vec![(source, 0.25), (destination, 0.75)],
        })
        .unwrap();
    // A reference keeps both lawful note streams continuously authored. The
    // two identical Sines feed the same destination gain after the cut.
    let mut reference = Editor::new(editor.project().clone(), 4).unwrap();
    reference
        .execute(Command::Gain {
            track: source,
            gain: 0.75,
        })
        .unwrap();
    reference
        .execute(Command::InsertInstrument {
            track: destination,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let predating_note = make_note(editor.project(), receiver_clip, 128, 6800, 84);
    editor.execute(predating_note).unwrap();
    let table = Arc::new(hex_engine::SineTable::new());
    let prepare = |editor: &Editor| {
        Arc::new(
            hex_engine::PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::clone(&table),
                None,
            )
            .unwrap(),
        )
    };
    let before = prepare(&editor);
    editor
        .execute(Command::MoveTrackDevices {
            source,
            destination,
            selected: DeviceSelectionIds::new(&[device]).unwrap(),
        })
        .unwrap();
    let after = prepare(&editor);
    let render_reference = |revision| {
        let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
        playback.apply(PlaybackCommand::Play);
        let mut pcm = vec![[0.0; 2]; FRAMES];
        playback.render(&mut pcm).unwrap();
        pcm
    };
    let old_pcm = render_reference(Arc::clone(&before));
    let next_pcm = render_reference(prepare(&reference));
    assert!(
        old_pcm[REQUEST..CUT]
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1e-4)
    );
    assert!(
        next_pcm[CUT..CUT + FADE]
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1e-4)
    );
    assert!(
        next_pcm[FRAMES - 128..]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    for partition in [1, BLOCK] {
        let (mut control, mut callback, predecessor, receipt) =
            begin_insert_with_pending_track(Arc::clone(&before), Arc::clone(&after), None, REQUEST);
        let mut pcm = vec![[0.0; 2]; FRAMES - REQUEST];
        let (fade_out, after_cut) = pcm.split_at_mut(FADE);
        for block in fade_out.chunks_mut(partition) {
            render(&mut callback, block);
            assert_eq!(receipt.state(), PublicationReceiptState::Pending);
            assert_eq!(control.status.recording_source(), Some(predecessor));
        }
        let (first, rest) = after_cut.split_at_mut(BLOCK);
        for block in first.chunks_mut(partition) {
            render(&mut callback, block);
        }
        assert_eq!(receipt.state(), PublicationReceiptState::Installed);
        let installed = control.status.recording_source().unwrap();
        assert_eq!(installed.output_identity, predecessor.output_identity);
        assert_eq!(installed.audio_revision, receipt.revision());
        assert_eq!(
            installed.context.editor_revision,
            predecessor.context.editor_revision + 1
        );
        let republished = (partition == BLOCK).then(|| {
            // The donor is still held when an ordinary publication inherits
            // the successor. Its audio receiver must stay on the destination.
            control.reclaim();
            control
                .publish(Playback::try_from_prepared_with_session(Arc::clone(&after)).unwrap())
                .unwrap()
        });
        for block in rest.chunks_mut(partition) {
            render(&mut callback, block);
        }
        if let Some(revision) = republished {
            assert_eq!(control.status.applied_revision.load(Acquire), revision);
        }
        for (offset, actual) in pcm.into_iter().enumerate() {
            let (reference, gain) = if offset < FADE {
                (
                    old_pcm[REQUEST + offset],
                    1.0 - (offset + 1) as f32 / FADE as f32,
                )
            } else {
                (
                    next_pcm[REQUEST + offset],
                    (offset - FADE + 1).min(FADE) as f32 / FADE as f32,
                )
            };
            for lane in 0..2 {
                assert!(
                    (actual[lane] - reference[lane] * gain).abs() < 2e-6,
                    "partition {partition}, frame {}, lane {lane}",
                    REQUEST + offset
                );
            }
        }
    }

    let (control, mut callback, predecessor, receipt) =
        begin_insert_with_pending_track(before, after, None, REQUEST);
    let mut prefix = [[0.0; 2]; 64];
    render(&mut callback, &mut prefix);
    assert_eq!(receipt.state(), PublicationReceiptState::Pending);
    assert_eq!(receipt.cancel(), FreshRuntimeCancellationResult::Cancelled);
    let mut restored = [[0.0; 2]; FADE + 64];
    for block in restored.chunks_mut(BLOCK) {
        render(&mut callback, block);
    }
    assert_eq!(control.status.recording_source(), Some(predecessor));
    let settled = REQUEST + prefix.len() + FADE;
    assert_eq!(restored[FADE..], old_pcm[settled..settled + 64]);
}

fn render(callback: &mut Callback, block: &mut [Frame]) {
    let counts = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(block).unwrap();
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
