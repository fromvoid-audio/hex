use super::*;
use gpui::TestAppContext;
use hex_lattice_contract::{DeviceParameterSnapshot, ParameterId, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use hex_session::device_gesture::GestureRequest;
use hex_session::service::Request;
use std::sync::Arc;

fn fixture(cx: &mut TestAppContext) -> (gpui::Entity<EditorView>, DeviceId, DeviceId) {
    let mut project = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = project
        .execute(Command::AddTrack {
            name: "test".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    let mut devices = Vec::new();
    for _ in 0..2 {
        let state = hex_engine::default_native_device_state(
            hex_lattice_contract::DeviceTypeId::from_static("hex.eq8"),
        )
        .unwrap();
        let Created::Device(device) = project
            .execute(Command::InsertDevice {
                track,
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!()
        };
        devices.push(device);
    }
    let (_, view) = super::super::super::clip_drag_ui_test::editor_window(
        cx,
        project.project().clone(),
        Arc::default(),
    );
    (view, devices[0], devices[1])
}
fn values() -> DeviceParameterSnapshot {
    DeviceParameterSnapshot::new(&[
        (ParameterId::from_static(18), ParameterValue::Float(1500.0)),
        (ParameterId::from_static(19), ParameterValue::Float(6.0)),
    ])
    .unwrap()
}
fn acknowledge_begin(view: &mut EditorView) {
    let begin = view.device_gesture.next_request().unwrap();
    view.device_gesture.mark_submitted();
    assert!(
        view.device_gesture
            .acknowledge(&hex_session::device_gesture::GestureAck {
                key: begin.key(),
                phase: begin.phase(),
                sequence: 0,
                accepted: true,
                preview_retained: false,
                reason: None
            })
    );
}

#[gpui::test]
fn snapshot_overlay_requires_exact_device_parameter_generation(cx: &mut TestAppContext) {
    let (editor, device, other) = fixture(cx);
    editor.update(cx, |view, _| {
        view.device_gesture
            .begin_snapshot(view.project_generation, device, values())
            .unwrap();
        for &(id, value) in values().entries() {
            assert_eq!(
                super::super::effective_parameter_value(
                    view,
                    super::super::ParameterKey {
                        device,
                        parameter: id
                    },
                    ParameterValue::Float(0.0)
                ),
                value
            );
            assert_eq!(
                super::super::effective_parameter_value(
                    view,
                    super::super::ParameterKey {
                        device: other,
                        parameter: id
                    },
                    ParameterValue::Float(0.0)
                ),
                ParameterValue::Float(0.0)
            );
        }
        view.project_generation += 1;
        assert_eq!(
            super::super::effective_parameter_value(
                view,
                super::super::ParameterKey {
                    device,
                    parameter: ParameterId::from_static(18)
                },
                ParameterValue::Float(0.0)
            ),
            ParameterValue::Float(0.0)
        );
        retire_invalid_owner(view, &HashSet::from([device, other]));
        assert!(view.device_gesture.has_pending_terminal());
    });
}

#[gpui::test]
fn snapshot_removal_cancels_without_cached_scalar_controls(cx: &mut TestAppContext) {
    let (editor, device, other) = fixture(cx);
    editor.update(cx, |view, _| {
        view.device_gesture
            .begin_snapshot(view.project_generation, device, values())
            .unwrap();
        view.device_panel.controls.clear();
        retire_invalid_owner(view, &HashSet::from([other]));
        acknowledge_begin(view);
        assert!(matches!(
            view.device_gesture.next_request(),
            Some(GestureRequest::Cancel { .. })
        ));
    });
}

#[gpui::test]
fn snapshot_save_barrier_finalizes_full_values_and_refusal_preserves_overlay(
    cx: &mut TestAppContext,
) {
    let (editor, device, _) = fixture(cx);
    editor.update(cx, |view, _| {
        view.device_gesture.begin_snapshot(view.project_generation, device, values()).unwrap();
        for _ in 0..super::super::super::UI_REQUEST_CAPACITY { view.device_gesture_barriers.push_back(Request::Undo); }
        assert!(!view.enqueue_device_barrier(Request::SaveCurrent));
        assert!(!view.device_gesture.has_pending_terminal());
        view.device_gesture_barriers.clear();
        assert!(view.enqueue_device_barrier(Request::SaveCurrent));
        assert!(view.device_gesture.has_pending_terminal());
        acknowledge_begin(view);
        assert!(matches!(view.device_gesture.next_request(), Some(GestureRequest::CommitSnapshot { snapshot, .. }) if snapshot == values()));
        assert!(matches!(view.device_gesture_barriers.front(), Some(Request::SaveCurrent)));
    });
}
