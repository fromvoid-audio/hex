//! A running filtered Vector has mutable phase and filter history to carry.
use super::*;
use hex_devices::operator_state::OPERATOR_TYPE_ID;
use hex_engine::NativeDeviceClass;
use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
use hex_lattice_contract::ParameterId;
use hex_project::{BeatTime, DeviceSelectionIds, TrackDeviceSlot};
use std::sync::atomic::AtomicBool;

#[test]
fn direct_vector_move_preserves_the_filtered_donor_and_new_note_boundary() {
    const REQUEST: usize = 512;
    const FADE: usize = 240;
    const CUT: usize = REQUEST + FADE;
    const BOUNDARY_END: usize = 4_096;
    // Leave room for the default global envelope scaling and filter drain.
    const FRAMES: usize = 32_768;
    let class = NativeDeviceClass::Instrument;
    let state = hex_engine::default_native_device_state_for_class(class, OPERATOR_TYPE_ID).unwrap();
    // The enabled filter forces the actual Operator mutable runtime, so a
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
    let mut reference = Editor::new(editor.project().clone(), 8).unwrap();

    // One uninterrupted runtime gets precisely the wanted notes for the
    // independent oracle. The graph being edited puts new notes on the receiver.
    for (frame, length, pitch) in [(CUT, 864, 69), (2600, 700, 74)] {
        let expected = note(reference.project(), donor_clip, frame, length, pitch);
        reference.execute(expected).unwrap();
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

    let sine = Arc::new(hex_engine::SineTable::new());
    let assets = ResidentDeviceAssets::default();
    let cancel = AtomicBool::new(false);
    let prepare = |project: &Project| {
        let resources =
            PreparedDeviceResources::prepare_on_control(&assets, project, 4096, &cancel).unwrap();
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare_with_device_resources(
                project,
                &BTreeMap::new(),
                Arc::clone(&sine),
                None,
                &hex_engine::audio_transform::AudioTransformPreparation::default(),
                &resources,
                0,
                &cancel,
            )
            .unwrap(),
        );
        (revision, resources)
    };
    let playback =
        |(revision, resources): &(Arc<hex_engine::PreparedRevision>, PreparedDeviceResources)| {
            Playback::try_from_prepared_with_device_resources(Arc::clone(revision), resources)
                .unwrap()
        };
    let before = prepare(editor.project());
    editor
        .execute(Command::MoveTrackDevices {
            source,
            destination,
            selected: DeviceSelectionIds::new(&[device]).unwrap(),
        })
        .unwrap();
    let after = prepare(editor.project());

    let reference = prepare(reference.project());
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
        let mut render = |block: &mut [Frame]| {
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
        };
        for block in fade_out.chunks_mut(partition) {
            render(block);
            assert_eq!(receipt.state(), PublicationReceiptState::Pending);
            assert_eq!(control.status.recording_source(), Some(donor));
        }
        for block in after_cut.chunks_mut(partition) {
            render(block);
            assert_eq!(receipt.state(), PublicationReceiptState::Installed);
        }
        for block in release_pcm.chunks_mut(BLOCK) {
            render(block);
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
    editor.execute(Command::RemoveMidiClip(donor_clip)).unwrap();
    let compound = prepare(editor.project());
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
