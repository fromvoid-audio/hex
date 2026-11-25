use super::*;
use gpui::TestAppContext;
fn next(service: &hex_session::service::Service) -> Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(2))
            }
            Err(error) => panic!("worker did not reply: {error}"),
        }
    }
}
#[gpui::test]
fn review_current_audition_worker_refusal_is_visible_but_stale_owner_is_ignored(
    cx: &mut TestAppContext,
) {
    let service = hex_session::service::Service::new(48_000, 8, 1_000_000).unwrap();
    let initial = next(&service);
    let (_, editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        initial.state.project.as_ref().clone(),
        Arc::default(),
    );
    let old = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(0));
    let active = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(0));
    editor.update(cx, |view, _| {
        view.state = Some(Arc::clone(&initial.state));
        view.settings.tuning.open = true;
        view.settings.tuning.audition_input = Some(Arc::clone(&active));
    });
    for (input, stale) in [(old, true), (Arc::clone(&active), false)] {
        service
            .submit(Request::PrepareTuningAudition {
                revision: 99,
                input,
            })
            .unwrap();
        let event = next(&service);
        assert!(event.error.is_some());
        assert!(
            event
                .tuning_audition_complete
                .as_ref()
                .unwrap()
                .is_cancelled()
        );
        editor.update(cx, |view, _| {
            view.receive(event);
            if stale {
                assert!(view.error.is_none());
                assert!(Arc::ptr_eq(
                    view.settings.tuning.audition_input.as_ref().unwrap(),
                    &active
                ));
            } else {
                assert!(view.settings.tuning.error.is_some());
                assert!(view.settings.tuning.audition_input.is_none());
            }
        });
    }
}
