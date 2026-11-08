use super::*;
use hex_lattice::{NativeInstrumentClock, NativeParameterAutomation};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

fn program() -> hex_lattice::PreparedParameterAutomation {
    let definition = hex_devices::fm4::device::FM4_DEFINITION;
    let state = definition.default_state().unwrap();
    let id = hex_devices::fm4::device::parameter::carrier(1);
    hex_lattice::PreparedParameterAutomation::new(
        vec![hex_lattice::ParameterAutomationLane {
            descriptor: *definition
                .descriptors()
                .iter()
                .find(|d| d.id == id)
                .unwrap(),
            semantics: definition
                .parameter_semantics_for_state(&state, id)
                .unwrap()
                .unwrap(),
            points: 0..2,
        }],
        vec![
            hex_lattice::ParameterAutomationPoint {
                beat_raw: 0,
                normalized: 0.0,
                step: false,
            },
            hex_lattice::ParameterAutomationPoint {
                beat_raw: 2 << 32,
                normalized: 1.0,
                step: false,
            },
        ],
    )
    .unwrap()
}
fn timing() -> crate::PreparedTempoMap {
    let project = hex_project::Project::new(48000).unwrap();
    crate::PreparedTempoMap::prepare(project.tempo_map().clone(), 48000).unwrap()
}
fn value(context: &SourceAutomation<'_>, at: usize) -> Option<ParameterValue> {
    context
        .block()
        .unwrap()
        .value_at(hex_devices::fm4::device::parameter::carrier(1), at)
}
#[test]
fn source_controls_follow_content_and_leave_performance_clock_independent() {
    let program = program();
    let timing = timing();
    let context = SourceAutomation::with_program(
        Some(&program),
        &timing,
        Origin {
            arrangement: Some(24000),
            performance: 96000,
            pass: 3,
            running: true,
        },
        64,
    );
    let musical = InstrumentClock {
        timing: &timing,
        start: 96000,
    };
    assert_eq!(musical.beat_at_offset(0), Some(4 << 32));
    assert_eq!(musical.arrangement_beat_at_offset(0), None);
    assert_eq!(value(&context, 0), Some(ParameterValue::Float(0.0)));
    let Some(ParameterValue::Float(end)) = value(&context, 48) else {
        panic!("float control")
    };
    assert!((end - 0.002).abs() < 0.000001);
    assert_eq!(
        context.clock.as_ref().unwrap().loop_pass_at_offset(48),
        Some(3)
    );
    // A Session launch/note origin changes no source-global control origin.
    let other_performance = SourceAutomation::with_program(
        Some(&program),
        &timing,
        Origin {
            arrangement: Some(24000),
            performance: 192000,
            pass: 3,
            running: true,
        },
        64,
    );
    assert_eq!(value(&context, 48), value(&other_performance, 48));
}
#[test]
fn stopped_cursor_freezes_and_unavailable_content_selects_base() {
    let program = program();
    let timing = timing();
    let origin = Origin {
        arrangement: Some(24000),
        performance: 96000,
        pass: 2,
        running: false,
    };
    let stopped = SourceAutomation::with_program(Some(&program), &timing, origin, 64);
    assert_eq!(value(&stopped, 0), Some(ParameterValue::Float(0.0)));
    assert_eq!(value(&stopped, 63), value(&stopped, 0));
    assert_eq!(
        stopped.clock.as_ref().unwrap().transport_is_running(),
        Some(false)
    );
    let unavailable = SourceAutomation::with_program(
        Some(&program),
        &timing,
        Origin {
            arrangement: None,
            ..origin
        },
        64,
    );
    assert_eq!(value(&unavailable, 0), None);
    assert_eq!(value(&unavailable, 63), None);
}
#[test]
fn loop_reset_and_generation_origin_are_independent_of_downstream_delay() {
    let program = program();
    let timing = timing();
    let origin = Origin {
        arrangement: Some(47999),
        performance: 100000,
        pass: 0,
        running: true,
    };
    let before = SourceAutomation::with_program(Some(&program), &timing, origin, 1);
    let wrapped = SourceAutomation::with_program(
        Some(&program),
        &timing,
        Origin {
            arrangement: Some(0),
            performance: 100001,
            pass: 1,
            running: true,
        },
        1,
    );
    let Some(ParameterValue::Float(before)) = value(&before, 0) else {
        panic!("float control")
    };
    assert!(before > 0.9999);
    assert_eq!(value(&wrapped, 0), Some(ParameterValue::Float(-1.0)));
    let generation = SourceAutomation::with_program(
        Some(&program),
        &timing,
        Origin {
            arrangement: Some(24000),
            ..origin
        },
        1,
    );
    let downstream = SourceAutomation::with_program(
        Some(&program),
        &timing,
        Origin {
            arrangement: Some(24128),
            ..origin
        },
        1,
    );
    assert_eq!(value(&generation, 0), Some(ParameterValue::Float(0.0)));
    assert_ne!(value(&generation, 0), value(&downstream, 0));
}
