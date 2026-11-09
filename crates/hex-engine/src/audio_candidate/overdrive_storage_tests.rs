//! Closed Overdrive preparation owns the real fixed-inline backing and delay.
use super::*;
use crate::{Audio, AudioSourceContext, PlaybackAction, PlaybackCommand, Renderer};
use hex_devices::overdrive::{OVERDRIVE_DEFINITION, OVERDRIVE_TYPE_ID, parameter};
use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceAutomationTarget, Editor, Project,
};
use std::collections::BTreeMap;

#[test]
fn overdrive_sealed_storage_and_compensated_render_match_the_ordinary_graph() {
    const RATE: u32 = 8_000;
    const FRAMES: usize = 97;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 4).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "overdrive".into(),
            path: "overdrive.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("audio fixture");
    };
    let state = OVERDRIVE_DEFINITION
        .default_state()
        .unwrap()
        .with_input_gain_millidb(-3000)
        .unwrap()
        .with_output_gain_millidb(1500)
        .unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("effect fixture");
    };
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: DeviceAutomationTarget::Parameter(parameter::DRY_WET),
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.8).unwrap(),
                outgoing: AutomationInterpolation::Step,
            }],
        })
        .unwrap();

    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("sealed Overdrive requires the supported constructor layout");
        return;
    }
    let audio = Audio::new(
        RATE,
        (0..FRAMES)
            .map(|frame| {
                let signal = (frame as f32 * 0.17).sin() * 0.24;
                [signal, -0.5 * signal]
            })
            .collect(),
    )
    .unwrap();
    let media = BTreeMap::from([(asset, audio.clone())]);
    let registry = hex_devices::native_registry().unwrap();
    let effects = AudioCandidateEffects::new(&registry);
    for bypassed in [false, true] {
        editor
            .execute(Command::SetDeviceBypass { device, bypassed })
            .unwrap();
        let project = editor.project();
        let graph = crate::compile::project(project, &media).unwrap();
        assert_eq!(graph.main_latency_frames(), 11);
        let mut renderer = Renderer::new(&graph);
        renderer.play();
        let mut expected = [[0.0; 2]; FRAMES];
        for block in expected.chunks_mut(13) {
            renderer.render(block).unwrap();
        }
        assert_eq!(expected[..11], [[0.0; 2]; 11]);
        assert!(expected[11..].iter().any(|frame| *frame != [0.0; 2]));

        let quote = effects.quote(project, 1).unwrap();
        let limits = quote.storage_reservation(FRAMES * 8).unwrap();
        let mut candidate = AudioCandidatePreparation::try_new_on_control(1, limits).unwrap();
        candidate
            .copy_resident_source_on_control(asset, &audio)
            .unwrap();
        effects
            .prepare(
                &mut candidate,
                project,
                2,
                AudioSourceContext {
                    project_generation: 1,
                    editor_revision: 8,
                },
                None,
            )
            .unwrap();
        let owner = candidate.owner_on_control().unwrap();
        let summary = owner
            .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                data_rows: quote.data_rows,
                handle_rows: quote.handle_rows,
                root_rows: 1,
                max_retained_bytes: limits.max_prepared_data_bytes,
                max_construction_peak_bytes: limits.max_construction_data_bytes,
            })
            .unwrap();
        assert_eq!(summary.data.unknown_storage, 0);
        owner
            .test_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
            .unwrap();
        let mut actual = [[0.0; 2]; FRAMES];
        for block in actual.chunks_mut(7) {
            owner.test_render(block).unwrap();
        }
        assert_eq!(actual, expected);
        candidate.cleanup_on_control().unwrap();
    }

    let malformed = NativeDeviceState::new(
        OVERDRIVE_TYPE_ID,
        OVERDRIVE_DEFINITION.state_version(),
        vec![0],
    )
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: malformed,
        })
        .unwrap();
    assert!(effects.quote(editor.project(), 1).is_err());
}
