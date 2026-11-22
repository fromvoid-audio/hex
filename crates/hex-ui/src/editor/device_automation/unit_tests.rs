use super::tests::project;
use super::*;
use hex_lattice_contract::ParameterId;
use hex_project::{Command, Created, Editor};

#[test]
fn parameter_identity_quantization_and_empty_presentation_preserve_canonical_state() {
    let (project, track, device, parameter) = project();
    let entry = Entry::read(
        &project,
        4,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::Parameter(parameter),
    )
    .unwrap();
    assert!(entry.target.current(&project, 5).is_none());
    assert!(
        Entry::read(
            &project,
            4,
            Owner::Main,
            device,
            DeviceAutomationTarget::Parameter(parameter)
        )
        .is_none()
    );
    assert!(entry.quantize(0.25).unwrap().is_finite());
    let boolean = Entry::read(
        &project,
        4,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::Parameter(ParameterId::from_static(5)),
    )
    .unwrap();
    assert_eq!(boolean.quantize(0.49), Some(0.0));
    assert_eq!(boolean.quantize(0.5), Some(1.0));
    let choice = Entry::read(
        &project,
        4,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::Parameter(ParameterId::from_static(4)),
    )
    .unwrap();
    let normalized = choice.quantize(0.4).unwrap();
    assert_eq!(choice.quantize(normalized), Some(normalized));
    assert!(entry.quantize(f32::NAN).is_none());
    let lanes = Lanes::default();
    assert!(!lanes.has_main(Owner::Track(track)));
    assert_eq!(lanes.extra_height(Owner::Track(track)), 0.0);
    assert!(project.device(device).unwrap().automation().is_empty());
    let mut model = Editor::new(project, 16).unwrap();
    model.execute(Command::RemoveDevice(device)).unwrap();
    assert!(entry.target.current(model.project(), 4).is_none());
}

#[test]
fn host_metadata_has_no_provider_id_and_keeps_identity_across_native_provider_change() {
    let (project, track, device, _) = project();
    let host = Entry::read(
        &project,
        4,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::HostInputGain,
    )
    .unwrap();
    assert_eq!(host.base, HostGain::ZERO.normalized());
    assert_eq!(host.quantize(host.base), Some(host.base));
    assert_eq!(host.quantize(1.01), None);
    assert_eq!(host.quantize(f32::NAN), None);
    assert_eq!(host.format(host.base), "0.000 db");
    assert_eq!(host.target.provider, None);
    assert!(
        Entry::read(
            &project,
            4,
            Owner::Track(track),
            device,
            DeviceAutomationTarget::Parameter(hex_engine::DEVICE_INPUT_GAIN.id)
        )
        .is_none()
    );
    let mut editor = Editor::new(project, 16).unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.overdrive",
            ))
            .unwrap(),
        })
        .unwrap();
    let refreshed = host.refreshed(editor.project(), 4).unwrap();
    assert_eq!(refreshed.target, host.target);
    assert!(refreshed.label.contains("overdrive"));
}

#[test]
fn receiving_fm4_exposes_granted_assignments_and_retires_unrouted_targets() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "carrier assignment".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_engine::default_native_device_state_for_class(
                NativeDeviceClass::Instrument,
                DeviceTypeId::from_static("hex.instrument.fm4"),
            )
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let source = editor.project().track(track).unwrap().source_id().unwrap();
    let before = editor.project().clone();
    let mut targets = Vec::new();
    for id in [2, 5, 8, 11, 29, 30, 31, 32] {
        let entry = Entry::read(
            editor.project(),
            4,
            Owner::Track(track),
            device,
            DeviceAutomationTarget::Parameter(ParameterId::from_static(id)),
        )
        .unwrap();
        assert!(entry.target.current(editor.project(), 4).is_some());
        assert!(entry.target.current(editor.project(), 5).is_none());
        targets.push(entry.target);
    }
    for id in [1, 28, 33, 36] {
        assert!(
            Entry::read(
                editor.project(),
                4,
                Owner::Track(track),
                device,
                DeviceAutomationTarget::Parameter(ParameterId::from_static(id))
            )
            .is_none()
        );
    }
    assert_eq!(editor.project(), &before, "discovery never authors a curve");
    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: None,
        })
        .unwrap();
    for target in targets {
        assert!(target.current(editor.project(), 4).is_none());
    }
}

#[test]
fn automation_display_scales_match_control_scales_for_db_bipolar_and_log_parameters() {
    let (base_project, track, device, gain_param) = project();
    // Utility Gain (0..=56.23413, LinearAmplitudeDecibels): 0 dB sits near 50%
    // center with comfortable vertical room down to -72 dB / -inf dB.
    let utility_gain = Entry::read(
        &base_project,
        1,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::Parameter(gain_param),
    )
    .unwrap();
    assert!(!utility_gain.is_display_linear());
    let unity_display = utility_gain.display_position(utility_gain.base);
    assert!(
        (unity_display - 0.5047).abs() < 1.0e-3,
        "utility 0 dB should sit near 50% center, got {unity_display}"
    );
    let db_to_canonical = |db: f32| {
        utility_gain
            .normalized(ParameterValue::Float(10.0_f32.powf(db / 20.0)))
            .unwrap()
    };
    let neg18 = utility_gain.display_position(db_to_canonical(-18.0));
    let neg36 = utility_gain.display_position(db_to_canonical(-36.0));
    let neg72 = utility_gain.display_position(db_to_canonical(-72.0));
    assert!((neg18 - 0.25).abs() < 1.0e-3, "-18 dB at 25%: {neg18}");
    assert!(neg36 > 0.15 && neg36 < neg18, "-36 dB keeps room: {neg36}");
    assert!((neg72 - 0.05).abs() < 1.0e-3, "-72 dB at 5%: {neg72}");
    assert_eq!(utility_gain.display_position(0.0), 0.0);
    assert_eq!(utility_gain.display_position(1.0), 1.0);
    // Round-trip display -> canonical -> display across the lane.
    for pos in [0.0, 0.05, 0.15, 0.25, 0.4, 0.5047, 0.75, 1.0] {
        let canonical = utility_gain.canonical_from_display(pos).unwrap();
        let back = utility_gain.display_position(canonical);
        assert!((back - pos).abs() < 1.0e-4, "pos={pos} back={back}");
    }

    // HostInputGain & HostOutputGain (-60..+24 dB): 0 dB sits at exact 50%.
    for kind in [
        DeviceAutomationTarget::HostInputGain,
        DeviceAutomationTarget::HostOutputGain,
    ] {
        let host = Entry::read(&base_project, 1, Owner::Track(track), device, kind).unwrap();
        assert!(!host.is_display_linear());
        assert!((host.display_position(host.base) - 0.5).abs() < 1.0e-4);
        assert_eq!(host.display_position(0.0), 0.0);
        assert_eq!(host.display_position(1.0), 1.0);
        let round_trip = host.canonical_from_display(0.5).unwrap();
        assert!((round_trip - HostGain::ZERO.normalized()).abs() < 1.0e-4);
    }

    // Saturator::OUTPUT (-36..+6 dB, asymmetric bipolar): 0 dB sits at exact 50%.
    let mut editor = Editor::new(base_project.clone(), 16).unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.saturator",
            ))
            .unwrap(),
        })
        .unwrap();
    let sat_output = Entry::read(
        editor.project(),
        1,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::Parameter(ParameterId::from_static(2)),
    )
    .unwrap();
    assert!(!sat_output.is_display_linear());
    assert!((sat_output.display_position(sat_output.base) - 0.5).abs() < 1.0e-4);
    assert!(sat_output.bipolar_zero_canonical().is_some());

    // EQ & EQ8 OUTPUT_GAIN (0..=16.0 linear amplitude): 0 dB sits near 57%.
    for type_id in ["hex.eq", "hex.eq8"] {
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: hex_engine::default_native_device_state(DeviceTypeId::from_static(type_id))
                    .unwrap(),
            })
            .unwrap();
        let eq_out = Entry::read(
            editor.project(),
            1,
            Owner::Track(track),
            device,
            DeviceAutomationTarget::Parameter(ParameterId::from_static(202)),
        )
        .unwrap();
        assert!(!eq_out.is_display_linear());
        let eq_unity = eq_out.display_position(eq_out.base);
        assert!(
            (eq_unity - 0.5708).abs() < 1.0e-3,
            "{type_id} 0 dB expected ~0.5708, got {eq_unity}"
        );
    }

    // Logarithmic parameter (Auto-Pan Rate) aligns with PositiveLogarithmic.
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.auto-pan",
            ))
            .unwrap(),
        })
        .unwrap();
    let rate = Entry::read(
        editor.project(),
        1,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::Parameter(ParameterId::from_static(2)),
    )
    .unwrap();
    assert!(rate.is_display_linear());
    let rate_mid = rate.canonical_from_display(0.5).unwrap();
    assert!((rate.display_position(rate_mid) - 0.5).abs() < 1.0e-5);
}

#[test]
fn utility_gain_pointer_position_and_curve_insertion_agree_on_curved_ramps() {
    let (project, track, device, gain_param) = project();
    let entry = Entry::read(
        &project,
        1,
        Owner::Track(track),
        device,
        DeviceAutomationTarget::Parameter(gain_param),
    )
    .unwrap();
    let viewport = super::super::viewport::Viewport {
        start: 0,
        length: 96_000,
    };
    let rect = [0.0, 10.0, 400.0, 64.0];
    let grid = super::super::musical_time::SnapGrid::Off;
    let p0 = AutomationPoint {
        beat: BeatTime::from_quarters(0).unwrap(),
        value: AutomationValue::new(entry.base).unwrap(),
        outgoing: AutomationInterpolation::Linear,
    };
    let p1 = AutomationPoint {
        beat: BeatTime::from_quarters(4).unwrap(),
        value: AutomationValue::new(0.0).unwrap(),
        outgoing: AutomationInterpolation::Linear,
    };
    let points = [p0, p1];
    // At quarter 2 (midpoint in time), true linear-amplitude interpolation gives
    // amplitude 0.5 (-6 dB), NOT display midpoint 0.25 (-18 dB).
    let expected_mid_canonical = 0.5 * entry.base;
    let expected_mid_y =
        rect[1] + (1.0 - entry.display_position(expected_mid_canonical)) * (rect[3] - 8.0) + 4.0;
    let inserted = paint::curve_insertion(
        &project,
        &entry,
        &points,
        grid,
        viewport,
        rect,
        [200.0, expected_mid_y],
    )
    .expect("hovering the true curved ramp inserts a point");
    assert!((inserted.value.get() - expected_mid_canonical).abs() < 1.0e-4);
    let pos = paint::point_position(&project, &entry, viewport, rect, &inserted).unwrap();
    assert!((pos[0] - 200.0).abs() < 0.5);
    assert!((pos[1] - expected_mid_y).abs() < 0.5);
    // Clicking halfway vertically between p0 and p1 in display space (~-18 dB)
    // misses the curve because the true linear-amplitude curve is at -6 dB.
    let straight_chord_y =
        rect[1] + (1.0 - 0.5 * entry.display_position(entry.base)) * (rect[3] - 8.0) + 4.0;
    assert!(
        paint::curve_insertion(
            &project,
            &entry,
            &points,
            grid,
            viewport,
            rect,
            [200.0, straight_chord_y]
        )
        .is_none(),
        "straight chord misses the true curved ramp"
    );
    // Dragging below 0 dB yields an audible attenuated value matching pointer Y.
    let dragged = paint::pointer_value(&project, &entry, grid, viewport, rect, [200.0, 56.0])
        .expect("pointer drag below 0 dB");
    assert!(dragged.value.get() > 0.0 && dragged.value.get() < entry.base);
    let dragged_pos = paint::point_position(&project, &entry, viewport, rect, &dragged).unwrap();
    assert!((dragged_pos[1] - 56.0).abs() < 0.5);
}
