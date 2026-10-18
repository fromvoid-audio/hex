use super::*;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};

fn devices(count: usize, filtered: bool, bypassed: bool) -> (Editor, hex_project::TrackId) {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let definition = &hex_devices::utility::UTILITY_DEFINITION;
    let mut state = definition.default_state().unwrap();
    if filtered {
        state = definition
            .patch_parameter(
                &state,
                hex_devices::utility::parameter::BASS_MONO,
                ParameterValue::Bool(true),
            )
            .unwrap();
    }
    state = state
        .with_input_gain_millidb(-6000)
        .unwrap()
        .with_output_gain_millidb(3000)
        .unwrap();
    for _ in 0..count {
        let Created::Device(device) = editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: state.clone(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        editor
            .execute(Command::SetDeviceBypass { device, bypassed })
            .unwrap();
    }
    (editor, track)
}
#[test]
fn utility_allowance_precedes_child_births_and_bounds_concrete_bodies_and_replacement_scratch() {
    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported Utility constructor allowance");
        return;
    }
    eprintln!(
        "SUPPORTED Utility constructor allowance actual filtered/bypassed/growth/empty paths"
    );
    for filtered in [false, true] {
        for bypassed in [false, true] {
            for (old_capacity, count) in [(0, 2), (1, 3), (7, 2), (7, 0)] {
                let (editor, track) = devices(count, filtered, bypassed);
                let source = editor.project().track(track).unwrap().devices();
                let mut custody = UtilityCustody::default();
                custody.entries.try_reserve_exact(old_capacity).unwrap();
                let old_capacity = custody.entries.capacity();
                let pointer = custody.entries.as_ptr();
                let original_counts: Vec<_> = source
                    .iter()
                    .map(|d| d.state().payload_storage_observation())
                    .collect();
                let owner = ProcessorChainOwner::Track(track.value());
                let quote = custody.construction_allowance(source, owner).unwrap();
                assert_eq!(custody.entries.as_ptr(), pointer);
                assert!(custody.entries.is_empty());
                assert_eq!(
                    source
                        .iter()
                        .map(|d| d.state().payload_storage_observation())
                        .collect::<Vec<_>>(),
                    original_counts
                );
                let chain = custody.prepare(source, owner, 48000).unwrap();
                let (body, header) = chain.parts().unwrap();
                let actual = custody.bytes(chain.arc()).unwrap() + body + header;
                let optional_filter = parts(OwnedShared::<
                    hex_devices::utility::PreparedUtilityFilterRuntime,
                >::declared_parts())
                .unwrap();
                assert_eq!(
                    quote.retained_bytes - actual,
                    if filtered { 0 } else { count * optional_filter }
                );
                let expected_scratch = count * size_of::<crate::PreparedEffectInstance>()
                    + if count > old_capacity {
                        old_capacity * size_of::<Entry>()
                    } else {
                        0
                    }
                    + if count == 0 {
                        0
                    } else {
                        hex_devices::utility::UTILITY_STATE_PAYLOAD_BYTES
                    };
                assert_eq!(custody.construction_overlap(), expected_scratch);
                assert_eq!(
                    quote.construction_peak_bytes - quote.retained_bytes,
                    expected_scratch
                );
                assert!(actual + custody.construction_overlap() <= quote.construction_peak_bytes);
                assert_eq!(custody.entries.capacity(), quote.entry_capacity);
                for entry in chain.iter() {
                    let configuration = entry.configuration.as_ref().unwrap();
                    assert_eq!(
                        configuration.payload().len(),
                        hex_devices::utility::UTILITY_STATE_PAYLOAD_BYTES
                    );
                    assert_eq!(configuration.input_gain_millidb(), -6000);
                    assert_eq!(configuration.output_gain_millidb(), 3000);
                    assert_eq!(entry.definition.is_some(), filtered);
                    assert_eq!(entry.bypassed, bypassed);
                }
                drop(chain);
                custody.clear();
                assert!(custody.entries.is_empty());
            }
        }
    }
}
#[test]
fn utility_allowance_refuses_invalid_state_without_mutating_reserved_destination() {
    let (mut editor, track) = devices(1, false, false);
    let device = editor.project().track(track).unwrap().devices()[0].id();
    let state = hex_lattice_contract::NativeDeviceState::new(
        hex_devices::utility::UTILITY_TYPE_ID,
        99,
        vec![0; hex_devices::utility::UTILITY_STATE_PAYLOAD_BYTES],
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    let mut custody = UtilityCustody::default();
    custody.entries.try_reserve_exact(7).unwrap();
    let pointer = custody.entries.as_ptr();
    let capacity = custody.entries.capacity();
    let source = editor.project().track(track).unwrap().devices();
    let owner = ProcessorChainOwner::Track(track.value());
    assert!(matches!(
        custody.construction_allowance(source, owner),
        Err(AudioStorageError::UnsupportedGraph)
    ));
    assert!(matches!(
        custody.prepare(source, owner, 48000),
        Err(AudioStorageError::UnsupportedGraph)
    ));
    assert!(custody.entries.is_empty());
    assert_eq!(custody.entries.as_ptr(), pointer);
    assert_eq!(custody.entries.capacity(), capacity);
    assert_eq!(custody.construction_overlap(), 0);
}
