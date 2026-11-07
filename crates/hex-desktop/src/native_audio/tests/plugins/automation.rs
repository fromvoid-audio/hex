//! Signed controller eligibility and ordinary Session authoring.
use super::*;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, DeviceAutomationTarget,
};

fn edit_with_running_callback(h: &mut ResidentHarness, request: Request) -> Event {
    h.service.submit(request).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render();
        if let Ok(event) = h.service.try_event() {
            return event;
        }
        assert!(Instant::now() < until, "vst3 lane edit did not complete");
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn signed_vst3_rows_admit_only_matching_arrangement_lane_edits() {
    const CHILD: &str = "HEX_DESKTOP_TEST_VST3_LANES_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        for variant in ["92", "81", "83", "84"] {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &name, "--ignored", "--nocapture"])
                .env(CHILD, variant)
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", variant)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "variant {variant}\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        return;
    }
    let variant = std::env::var(CHILD).unwrap();
    let _native = native_fixture();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    let initial = h.next();
    h.applied(0);
    let insert = request(&initial.state, PluginEffectDestination::Main);
    let exact_class = insert.selection.cid;
    h.run(Request::InsertPluginEffect(insert));
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    let (applied, metadata) = h.applied(1);
    let device = accepted.state.project.main_devices()[0].id();
    let query = hex_session::service::PluginParameterQuery {
        context: metadata.context,
        device,
        output_identity: h.audio.as_ref().unwrap().control.output_identity().unwrap(),
        request_id: 1,
    };
    assert_eq!(
        h.run(Request::QueryPluginParameters(query)).state.revision,
        accepted.state.revision
    );
    let observed = h.next();
    assert_eq!(observed.state.project, accepted.state.project);
    let rows = observed.plugin_parameters.unwrap().rows.unwrap();
    let [row] = rows.as_slice() else {
        panic!("expected one SDK parameter")
    };
    let eligible = variant == "92" || variant == "81";
    assert_eq!(row.automation_class, eligible.then_some(exact_class));
    assert_eq!(row.editable, eligible);
    assert_eq!(row.id, if variant == "92" { 0 } else { 7 });
    assert_eq!(row.steps, if variant == "81" { 3 } else { 0 });

    let target = DeviceAutomationTarget::Vst3 { parameter: row.id };
    let point = |beat, value, outgoing| AutomationPoint {
        beat,
        value: AutomationValue::new(value).unwrap(),
        outgoing,
    };
    let points = vec![
        point(
            BeatTime::ZERO,
            0.0,
            if row.steps == 0 {
                AutomationInterpolation::Linear
            } else {
                AutomationInterpolation::Step
            },
        ),
        point(
            BeatTime::from_ratio(1, 1).unwrap(),
            1.0,
            AutomationInterpolation::Step,
        ),
    ];
    let edit = |revision, vst3_class, points| Request::DeviceAutomation {
        project_generation: metadata.context.project_generation,
        revision,
        device,
        target,
        vst3_class,
        points,
    };
    let revision = accepted.state.revision;
    for refused in [
        edit(revision - 1, Some(exact_class), points.clone()),
        edit(revision, Some([0xa3; 16]), points.clone()),
    ] {
        h.service.submit(refused).unwrap();
        let refused = h.next();
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, accepted.state.project);
        assert_eq!(refused.state.revision, revision);
    }
    let event =
        edit_with_running_callback(&mut h, edit(revision, Some(exact_class), points.clone()));
    if !eligible {
        assert!(
            event.error.is_some(),
            "unsupported SDK parameter was admitted"
        );
        assert_eq!(event.state.project, accepted.state.project);
        assert_eq!(event.state.revision, revision);
        return;
    }
    assert!(event.error.is_none(), "{:?}", event.error);
    assert_eq!(event.state.revision, revision + 1);
    let mut applied = h.applied(applied).0;
    let lane = event
        .state
        .project
        .device(device)
        .unwrap()
        .automation_target(target)
        .unwrap();
    assert_eq!(lane.vst3_class, Some(exact_class));
    assert_eq!(lane.points, points);

    let undone = edit_with_running_callback(&mut h, Request::Undo);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project, accepted.state.project);
    applied = h.applied(applied).0;
    let redone = edit_with_running_callback(&mut h, Request::Redo);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(redone.state.project, event.state.project);
    h.applied(applied);

    let files = Files::new();
    let path = files.0.join("authored-vst3.hex");
    hex_storage::save_new(&path, &redone.state.project).unwrap();
    assert_eq!(hex_storage::load(&path).unwrap(), *redone.state.project);
}
