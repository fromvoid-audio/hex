//! Closed preparation uses the original Delay owner and its private ring capacities.
use super::host_output_modulation_tests::compare_native_and_sealed_pcm;
use crate::Audio;
use hex_devices::delay::{DEFINITION, DelayDefinition, DelayRuntime, TYPE_ID};
use hex_lattice::{OwnedProcessorRuntime, PreparedOwnedProcessorRuntime};
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState};
use hex_project::{Command, Created, Editor, Project};

#[test]
fn sealed_delay_retains_the_original_stereo_echo_law() {
    const RATE: u32 = 8_000;
    const FRAMES: usize = 259;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 4).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "impulse".into(),
            path: "delay-impulse.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture");
    };
    // Exact v1 state. The closed native constructor must use the same
    // migration as the ordinary registry, including its free stereo timing.
    let state = NativeDeviceState::new(
        TYPE_ID,
        1,
        [1.0_f32, 0.5, 1.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect(),
    )
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap();
    let mut input = vec![[0.0; 2]; FRAMES];
    input[0] = [0.5, -0.25];
    let audio = Audio::new(RATE, input).unwrap();

    compare_native_and_sealed_pcm::<FRAMES>(editor.project(), asset, &audio, |pcm| {
        for (index, frame) in pcm.iter().enumerate() {
            let expected = if index > 0 && index % 8 == 0 {
                let attenuation = 0.5_f32.powi(index as i32 / 8 - 1);
                [0.5 * attenuation, -0.25 * attenuation]
            } else {
                [0.0; 2]
            };
            assert_eq!(*frame, expected, "frame {index}");
        }
    });
}

#[test]
fn closed_delay_charges_both_real_rings_and_the_transaction_box() {
    let state = DEFINITION.default_state().unwrap();
    for rate in [8_000, 48_000, 192_000] {
        let factory = DelayDefinition::prepare_owned(&state, rate, &[]).unwrap();
        let runtime = factory.create_runtime().unwrap();
        let actual = runtime.as_any().downcast_ref::<DelayRuntime>().unwrap();
        let (inline, rings) = actual.control_storage().unwrap();
        assert_eq!(inline, std::mem::size_of::<DelayRuntime>());
        let one_ring = (rate as usize * 16 + 4) * std::mem::size_of::<[f32; 2]>();
        assert_eq!(rings, [one_ring; 2]);
        assert_eq!(
            actual.fixed_storage_pair_bytes(),
            Some(inline + 2 * one_ring)
        );
        assert_eq!(factory.declared_pair_bytes(), inline + 2 * one_ring);
    }
}
