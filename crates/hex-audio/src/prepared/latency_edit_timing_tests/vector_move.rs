//! A running filtered Vector has mutable phase and filter history to carry.
use super::*;
use hex_devices::operator_state::OPERATOR_TYPE_ID;
use hex_engine::NativeDeviceClass;
use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
use hex_lattice_contract::ParameterId;
use hex_project::{BeatTime, ClipId, DeviceId, DeviceSelectionIds, TrackDeviceSlot, TrackId};
use std::sync::atomic::AtomicBool;

const REQUEST: usize = 512;
const FADE: usize = 240;
const CUT: usize = REQUEST + FADE;
const BOUNDARY_END: usize = 4_096;
// Leave room for the default global envelope scaling and filter drain.
const FRAMES: usize = 32_768;

struct VectorMoveScene {
    before: Project,
    after: Project,
    reference: Project,
    original: TrackId,
    receiver: TrackId,
    device: DeviceId,
    donor_clip: ClipId,
}

fn scene(return_cut: Option<usize>) -> VectorMoveScene {
    let class = NativeDeviceClass::Instrument;
    let state = hex_engine::default_native_device_state_for_class(class, OPERATOR_TYPE_ID).unwrap();
    // The enabled filter forces the actual Vector mutable runtime, so a
    // fresh oscillator and filter cannot reproduce the settled donor phase.
    let state = hex_engine::patch_native_device_parameter_for_class(
        class,
        &state,
        ParameterId::from_static(20_001),
        ParameterValue::Bool(true),
    )
    .unwrap();
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Track(source) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "source".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state,
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
        panic!("destination");
    };
    let mut place = |track| {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(2).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip");
        };
        clip
    };
    let donor_clip = place(source);
    let receiver_clip = place(destination);
    let note = |project: &Project, clip, frame: usize, frames: usize, pitch| {
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
    let donor = note(editor.project(), donor_clip, 0, 1950, 57);
    editor.execute(donor).unwrap();
    if let Some(frame) = return_cut {
        let at_return = note(editor.project(), donor_clip, frame, 590, 76);
        editor.execute(at_return).unwrap();
    }
    let mut reference = Editor::new(editor.project().clone(), 8).unwrap();

    // One uninterrupted runtime gets precisely the wanted notes for the
    // independent oracle. The graph being edited puts new notes on the receiver.
    for (frame, length, pitch) in [(CUT, 864, 69), (2600, 700, 74)] {
        if return_cut.is_none_or(|cut| frame < cut) {
            let expected = note(reference.project(), donor_clip, frame, length, pitch);
            reference.execute(expected).unwrap();
        }
        let actual = note(editor.project(), receiver_clip, frame, length, pitch);
        editor.execute(actual).unwrap();
    }
    let predating = note(editor.project(), receiver_clip, 128, 5_800, 84);
    editor.execute(predating).unwrap();
    editor
        .execute(Command::SetTrackGains {
            gains: vec![(source, 0.25), (destination, 0.75)],
        })
        .unwrap();

    let before = editor.project().clone();
    move_vector(&mut editor, device, source, destination);
    VectorMoveScene {
        before,
        after: editor.project().clone(),
        reference: reference.project().clone(),
        original: source,
        receiver: destination,
        device,
        donor_clip,
    }
}

fn move_vector(editor: &mut Editor, device: DeviceId, source: TrackId, destination: TrackId) {
    editor
        .execute(Command::MoveTrackDevices {
            source,
            destination,
            selected: DeviceSelectionIds::new(&[device]).unwrap(),
        })
        .unwrap();
}

type PreparedVector = (Arc<hex_engine::PreparedRevision>, PreparedDeviceResources);

fn prepare(project: &Project, sine: &Arc<hex_engine::SineTable>) -> PreparedVector {
    let assets = ResidentDeviceAssets::default();
    let cancel = AtomicBool::new(false);
    let resources =
        PreparedDeviceResources::prepare_on_control(&assets, project, 4096, &cancel).unwrap();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare_with_device_resources(
            project,
            &BTreeMap::new(),
            Arc::clone(sine),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            0,
            &cancel,
        )
        .unwrap(),
    );
    (revision, resources)
}

fn playback((revision, resources): &PreparedVector) -> Playback {
    Playback::try_from_prepared_with_device_resources(Arc::clone(revision), resources).unwrap()
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

#[test]
fn direct_vector_move_preserves_the_filtered_donor_and_new_note_boundary() {
    let scene = scene(None);
    let sine = Arc::new(hex_engine::SineTable::new());
    let before = prepare(&scene.before, &sine);
    let after = prepare(&scene.after, &sine);
    let reference = prepare(&scene.reference, &sine);
    for partition in [1, BLOCK] {
        let mut oracle = playback(&reference);
        oracle.apply(PlaybackCommand::Play);
        let mut expected = vec![[0.0; 2]; FRAMES];
        let (prefix, remainder) = expected.split_at_mut(REQUEST);
        let (boundary, release) = remainder.split_at_mut(BOUNDARY_END - REQUEST);
        let (fade, successor) = boundary.split_at_mut(FADE);
        oracle.render(prefix).unwrap();
        for block in fade
            .chunks_mut(partition)
            .chain(successor.chunks_mut(partition))
            .chain(release.chunks_mut(BLOCK))
        {
            oracle.render(block).unwrap();
        }
        for range in [REQUEST..CUT, CUT..CUT + FADE] {
            assert!(
                expected[range]
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 1e-4)
            );
        }
        assert!(
            expected[FRAMES - 64..]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );

        let (control, mut callback, donor, receipt) = begin_quantized_playback_insert(
            playback(&before),
            playback(&after),
            None,
            REQUEST,
            LaunchQuantization::ThirtySecond,
        );
        let mut pcm = vec![[0.0; 2]; FRAMES - REQUEST];
        let (boundary_pcm, release_pcm) = pcm.split_at_mut(BOUNDARY_END - REQUEST);
        let (fade_out, after_cut) = boundary_pcm.split_at_mut(FADE);
        for block in fade_out.chunks_mut(partition) {
            render(&mut callback, block);
            assert_eq!(receipt.state(), PublicationReceiptState::Pending);
            assert_eq!(control.status.recording_source(), Some(donor));
        }
        for block in after_cut.chunks_mut(partition) {
            render(&mut callback, block);
            assert_eq!(receipt.state(), PublicationReceiptState::Installed);
        }
        for block in release_pcm.chunks_mut(BLOCK) {
            render(&mut callback, block);
        }
        for (offset, (actual, expected)) in pcm.iter().zip(&expected[REQUEST..]).enumerate() {
            let gain = if offset < FADE {
                0.25 * (1.0 - (offset + 1) as f32 / FADE as f32)
            } else {
                0.75 * (offset - FADE + 1).min(FADE) as f32 / FADE as f32
            };
            for (actual, expected) in actual.iter().zip(expected) {
                let expected = expected * gain;
                assert!(
                    (actual - expected).abs() < 2e-6,
                    "partition {partition}, frame {}: {actual} vs {expected}",
                    REQUEST + offset
                );
            }
        }
        let installed = control.status.recording_source().unwrap();
        assert_eq!(installed.audio_revision, receipt.revision());
        assert_eq!(installed.output_identity, donor.output_identity);
        assert_eq!(
            installed.context.editor_revision,
            donor.context.editor_revision + 1
        );
    }

    // A simultaneous donor clip removal cannot use the original gate proof.
    let mut editor = Editor::new(scene.after, 8).unwrap();
    editor
        .execute(Command::RemoveMidiClip(scene.donor_clip))
        .unwrap();
    let compound = prepare(editor.project(), &sine);
    let (_control, mut callback, _, receipt) = begin_quantized_playback_insert(
        playback(&before),
        playback(&compound),
        None,
        REQUEST,
        LaunchQuantization::ThirtySecond,
    );
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 1]).unwrap();
    assert!(matches!(
        receipt.state(),
        PublicationReceiptState::Installed | PublicationReceiptState::Refused(_)
    ));
}

#[test]
fn reversing_a_vector_move_carries_both_already_sounding_notes() {
    const RETURN_REQUEST: usize = 1_200;
    const RETURN_CUT: usize = RETURN_REQUEST + FADE;
    let scene = scene(Some(RETURN_CUT));
    let sine = Arc::new(hex_engine::SineTable::new());
    let before = prepare(&scene.before, &sine);
    let after = prepare(&scene.after, &sine);
    let mut editor = Editor::new(scene.after, 8).unwrap();
    move_vector(&mut editor, scene.device, scene.receiver, scene.original);
    let returned = prepare(editor.project(), &sine);
    let reference = prepare(&scene.reference, &sine);
    let (mut control, mut callback, _, forward) = begin_quantized_playback_insert(
        playback(&before),
        playback(&after),
        None,
        REQUEST,
        LaunchQuantization::ThirtySecond,
    );
    let mut pcm = vec![[0.0; 2]; FRAMES - REQUEST];
    let (outgoing, returning) = pcm.split_at_mut(RETURN_REQUEST - REQUEST);
    let (departure_fade, receiver_pcm) = outgoing.split_at_mut(FADE);
    render(&mut callback, departure_fade);
    assert_eq!(forward.state(), PublicationReceiptState::Pending);
    for block in receiver_pcm.chunks_mut(BLOCK) {
        render(&mut callback, block);
    }
    assert_eq!(forward.state(), PublicationReceiptState::Installed);

    control.reclaim();
    let predecessor = control.status.recording_source().unwrap();
    assert_eq!(predecessor.audio_revision, forward.revision());
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let inverse = control
        .publish_unaccounted_pcm_obligation(
            playback(&returned),
            PublicationPolicy::DeviceInsert { predecessor, fence },
            predecessor.context.project_generation,
            true,
            false,
            Some(hex_engine::AudioSourceContext {
                editor_revision: predecessor.context.editor_revision + 1,
                ..predecessor.context
            }),
        )
        .unwrap();
    let receipt = control.fresh_runtime_receipt(inverse).unwrap();
    let (return_fade, restored_pcm) = returning.split_at_mut(FADE);
    render(&mut callback, return_fade);
    let faded = (receipt.state(), control.status.recording_source());
    let (first_restored, remaining) = restored_pcm.split_at_mut(BLOCK);
    render(&mut callback, first_restored);
    let after_cut = receipt.state();
    for block in remaining.chunks_mut(BLOCK) {
        render(&mut callback, block);
    }
    let restored = control.status.recording_source().unwrap();

    let mut oracle = playback(&reference);
    oracle.apply(PlaybackCommand::Play);
    let mut expected = vec![[0.0; 2]; FRAMES];
    let (prefix, moves) = expected.split_at_mut(REQUEST);
    oracle.render(prefix).unwrap();
    let (forward_pcm, reverse_pcm) = moves.split_at_mut(RETURN_REQUEST - REQUEST);
    let (forward_fade, on_receiver) = forward_pcm.split_at_mut(FADE);
    let (reverse_fade, on_original) = reverse_pcm.split_at_mut(FADE);
    for block in [forward_fade, on_receiver, reverse_fade, on_original]
        .into_iter()
        .flat_map(|audio| audio.chunks_mut(BLOCK))
    {
        oracle.render(block).unwrap();
    }
    assert!(
        expected[RETURN_REQUEST..RETURN_CUT]
            .iter()
            .flatten()
            .any(|s| s.abs() > 1e-4)
    );
    assert!(
        expected[FRAMES - 64..]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    let first_mismatch = pcm.iter().zip(&expected[REQUEST..]).enumerate().find_map(
        |(offset, (actual, expected))| {
            let frame = REQUEST + offset;
            let fade_in = |frames: usize| frames.min(FADE) as f32 / FADE as f32;
            let gain = match frame {
                ..CUT => 0.25 * (1.0 - fade_in(frame - REQUEST + 1)),
                CUT..RETURN_REQUEST => 0.75 * fade_in(frame - CUT + 1),
                RETURN_REQUEST..RETURN_CUT => 0.75 * (1.0 - fade_in(frame - RETURN_REQUEST + 1)),
                _ => 0.25 * fade_in(frame - RETURN_CUT + 1),
            };
            (0..2).find_map(|lane| {
                let wanted = expected[lane] * gain;
                let difference = (actual[lane] - wanted).abs();
                (!difference.is_finite() || difference >= 2e-6).then_some((
                    frame,
                    lane,
                    actual[lane],
                    wanted,
                ))
            })
        },
    );
    assert_eq!(
        (faded, after_cut, receipt.state(), first_mismatch),
        (
            (PublicationReceiptState::Pending, Some(predecessor)),
            PublicationReceiptState::Installed,
            PublicationReceiptState::Installed,
            None
        ),
    );
    assert_eq!(restored.audio_revision, inverse);
    assert_eq!(restored.output_identity, predecessor.output_identity);
}
