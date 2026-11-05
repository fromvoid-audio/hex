//! Public quote plus actual original constructor DATA, including legacy migration.
use super::*;
use crate::{Audio, AudioSourceContext};
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState};
use hex_project::{Command, Created, Editor, Project};
#[test]
fn reverb_public_quote_covers_each_real_ring_allocation_and_versioned_state_migration() {
    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("reverb native constructor: unsupported layout, not exercised");
        return;
    }
    for legacy in [false, true] {
        let mut edit = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
        let Created::Imported { track, asset, .. } = edit
            .execute(Command::ImportAudioTrack {
                name: "room".into(),
                path: "original.wav".into(),
                sample_rate: 48000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("import");
        };
        let state = if legacy {
            NativeDeviceState::new(
                hex_devices::reverb::REVERB_TYPE_ID,
                1,
                [2.1_f32, 73.0]
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
                    .collect(),
            )
            .unwrap()
        } else {
            hex_devices::reverb::REVERB_DEFINITION
                .default_state()
                .unwrap()
        };
        edit.execute(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        })
        .unwrap();
        let Created::ReturnTrack(bus) = edit
            .execute(Command::AddReturnTrack {
                name: "room return".into(),
            })
            .unwrap()
        else {
            panic!("return");
        };
        edit.execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state,
        })
        .unwrap();
        edit.execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.4,
        })
        .unwrap();
        AudioCandidateEffects::with_host_definitions(&[], |effects| {
            let quote = effects.quote(edit.project(), 1).unwrap();
            let limits = quote.storage_reservation(8 * 8).unwrap();
            let mut preparation = AudioCandidatePreparation::try_new_on_control(1, limits).unwrap();
            preparation
                .copy_resident_source_on_control(
                    asset,
                    &Audio::new(48000, vec![[0.25, -0.1]; 8]).unwrap(),
                )
                .unwrap();
            effects
                .prepare(
                    &mut preparation,
                    edit.project(),
                    11,
                    AudioSourceContext {
                        project_generation: 3,
                        editor_revision: 7,
                    },
                    None,
                )
                .unwrap();
            let summary = preparation
                .owner_on_control()
                .unwrap()
                .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                    data_rows: quote.data_rows,
                    handle_rows: quote.handle_rows,
                    root_rows: 8,
                    max_retained_bytes: limits.max_prepared_data_bytes,
                    max_construction_peak_bytes: limits.max_construction_data_bytes,
                })
                .unwrap();
            assert_eq!(summary.data.unknown_storage, 0);
            assert!(
                summary.data.data_allocations
                    >= 4 * (1 + hex_devices::reverb::RUNTIME_HEAP_ALLOCATIONS)
            );
            assert!(summary.data.data_allocations <= quote.data_rows);
            assert!(summary.owned_runtime_bytes >= 4 * 240_000);
            preparation.cleanup_on_control().unwrap();
            assert!(!preparation.failed_incomplete());
            Ok(())
        })
        .unwrap();
    }
}
