//! Direct authored-instrument removal must stop its actual DSP at the cut.
use super::*;
use hex_engine::NativeDeviceClass;
use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
use hex_lattice_contract::ParameterId;
use hex_project::{BeatTime, TrackDeviceSlot};
use std::sync::atomic::AtomicBool;

#[test]
fn removing_a_held_vector_fades_its_original_pcm_and_keeps_the_bed() {
    const REQUEST: usize = 512;
    const FADE: usize = 240;
    const BED: Frame = [0.125, -0.0625];
    let (mut editor, _, _) = resident_input(vec![BED; 2048]);
    let asset = editor.project().assets().next().unwrap().id();
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(RATE, vec![BED; 2048]).unwrap(),
    )]);
    let class = NativeDeviceClass::Instrument;
    let state = hex_engine::default_native_device_state_for_class(
        class,
        hex_devices::operator_state::OPERATOR_TYPE_ID,
    )
    .unwrap();
    let state = hex_engine::patch_native_device_parameter_for_class(
        class,
        &state,
        ParameterId::from_static(20_001),
        ParameterValue::Bool(true),
    )
    .unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "vector".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state,
        })
        .unwrap()
    else {
        panic!("instrument track");
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("instrument clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 57,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let original = editor.project().clone();
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    let removed = editor.project();
    let assets = ResidentDeviceAssets::default();
    let sine = Arc::new(hex_engine::SineTable::new());
    let cancel = AtomicBool::new(false);
    let prepare = |project: &Project| {
        let resources =
            PreparedDeviceResources::prepare_on_control(&assets, project, 4096, &cancel).unwrap();
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare_with_device_resources(
                project,
                &media,
                Arc::clone(&sine),
                None,
                &hex_engine::audio_transform::AudioTransformPreparation::default(),
                &resources,
                0,
                &cancel,
            )
            .unwrap(),
        );
        Playback::try_from_prepared_with_device_resources(revision, &resources).unwrap()
    };
    for partition in [1, BLOCK] {
        let mut reference = prepare(&original);
        reference.apply(PlaybackCommand::Play);
        reference.render(&mut [[0.0; 2]; REQUEST]).unwrap();
        let mut sustaining = [[0.0; 2]; FADE + 64];
        for output in sustaining[..FADE].chunks_mut(partition) {
            reference.render(output).unwrap();
        }
        reference.render(&mut sustaining[FADE..]).unwrap();
        assert!(sustaining[FADE..].iter().any(|frame| *frame != BED));

        let (control, mut callback, predecessor, receipt) = begin_quantized_playback_insert(
            prepare(&original),
            prepare(removed),
            None,
            REQUEST,
            LaunchQuantization::ThirtySecond,
        );
        let mut actual = [[0.0; 2]; FADE + 64];
        for output in actual[..FADE].chunks_mut(partition) {
            render(&mut callback, output);
            assert_eq!(receipt.state(), PublicationReceiptState::Pending);
            assert_eq!(control.status.recording_source(), Some(predecessor));
        }
        render(&mut callback, &mut actual[FADE..]);
        assert_eq!(receipt.state(), PublicationReceiptState::Installed);
        let installed = control.status.recording_source().unwrap();
        assert_eq!(installed.audio_revision, receipt.revision());
        assert_eq!(installed.output_identity, predecessor.output_identity);
        assert!(actual[FADE..].iter().all(|frame| *frame == BED));
        for (index, (actual, source)) in actual[..FADE].iter().zip(sustaining).enumerate() {
            let gain = 1.0 - (index + 1) as f32 / FADE as f32;
            for lane in 0..2 {
                let expected = BED[lane] + (source[lane] - BED[lane]) * gain;
                assert!((actual[lane] - expected).abs() < 2e-6);
            }
        }
    }

    let (control, mut callback, predecessor, receipt) = begin_quantized_playback_insert(
        prepare(&original),
        prepare(removed),
        None,
        REQUEST,
        LaunchQuantization::ThirtySecond,
    );
    let mut reference = prepare(&original);
    reference.apply(PlaybackCommand::Play);
    reference.render(&mut [[0.0; 2]; REQUEST]).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    reference.render(&mut prefix).unwrap();
    render(&mut callback, &mut prefix);
    assert_eq!(receipt.state(), PublicationReceiptState::Pending);
    assert_eq!(receipt.cancel(), FreshRuntimeCancellationResult::Cancelled);
    let mut recovered = [[0.0; 2]; FADE];
    let mut expected = [[0.0; 2]; FADE];
    reference.render(&mut expected).unwrap();
    render(&mut callback, &mut recovered);
    assert_eq!(receipt.state(), PublicationReceiptState::Cancelled);
    assert_eq!(control.status.recording_source(), Some(predecessor));
    let starting_gain = 1.0 - prefix.len() as f32 / FADE as f32;
    for (index, (actual, source)) in recovered.iter().zip(expected).enumerate() {
        let gain = starting_gain + (1.0 - starting_gain) * (index + 1) as f32 / FADE as f32;
        for lane in 0..2 {
            let expected = BED[lane] + (source[lane] - BED[lane]) * gain;
            assert!((actual[lane] - expected).abs() < 2e-6);
        }
    }
}

fn render(callback: &mut Callback, output: &mut [Frame]) {
    let measured = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(output).unwrap();
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current,
        ),
        (0, 0, 0, 0)
    );
}
