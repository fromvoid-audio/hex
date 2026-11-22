//! Publish a stopped VST3 lane edit after Play has started on the original worker.
use super::*;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, DeviceAutomationTarget, DeviceId,
};
use hex_session::service::PluginParameterQuery;

#[test]
#[ignore = "requires explicit source-bound signed SDK receipts"]
fn play_started_during_instrument_lane_edit_keeps_the_original_worker() {
    instrument_entrypoint(InstrumentWorkflow::AuthoringAcrossPlay);
}

pub(super) fn verify(h: &mut ResidentHarness, device: DeviceId, applied: u64) {
    let context = h.submitted.unwrap().1.context;
    let output_identity = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let query = PluginParameterQuery {
        context,
        device,
        output_identity,
        request_id: 1,
        page_first: None,
    };
    h.run(Request::QueryPluginParameters(query));
    let parameter = h.next().plugin_parameters.unwrap();
    let original_binding = parameter.source.unwrap();
    let rows = parameter.rows.unwrap();
    let row = rows.iter().find(|row| row.id == 7).unwrap();
    let target = DeviceAutomationTarget::Vst3 { parameter: row.id };
    let edit = |revision, points| Request::DeviceAutomation {
        project_generation: context.project_generation,
        revision,
        device,
        target,
        vst3_class: row.automation_class,
        points,
    };
    let point = |beat, normalized| AutomationPoint {
        beat,
        value: AutomationValue::new(normalized).unwrap(),
        outgoing: AutomationInterpolation::Step,
    };
    let first = point(BeatTime::ZERO, 0.75);
    let (ready, resume, _) = h.probe_next_native_publication();
    h.service
        .submit(edit(context.editor_revision, vec![first]))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while ready.try_recv().is_err() {
        h.pump();
        h.render_worker_step();
        if let Ok(event) = h.service.try_event() {
            panic!(
                "instrument edit ended before publication: {:?}",
                event.error
            );
        }
        assert!(Instant::now() < until, "compiled instrument automation");
        std::thread::yield_now();
    }
    let audio = h.audio.as_mut().unwrap();
    assert!(!audio.status().unwrap().playing.load(Acquire));
    audio.control.command(PlaybackCommand::Play).unwrap();
    h.render_worker_step();
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    resume.send(()).unwrap();
    let committed = h.next();
    assert!(committed.error.is_none(), "{:?}", committed.error);
    assert_eq!(
        committed
            .state
            .project
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap()
            .points,
        [first]
    );
    let (applied, metadata) = h.applied(applied);
    assert_eq!(metadata.context.editor_revision, committed.state.revision);

    let next = vec![first, point(BeatTime::from_ratio(1, 8).unwrap(), 0.5)];
    let edited = super::super::automation::edit_with_running_callback(
        h,
        edit(committed.state.revision, next.clone()),
    );
    assert!(edited.error.is_none(), "{:?}", edited.error);
    assert_eq!(
        edited
            .state
            .project
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap()
            .points,
        next
    );
    let (_, metadata) = h.applied(applied);
    h.run(Request::QueryPluginParameters(PluginParameterQuery {
        context: metadata.context,
        request_id: 2,
        ..query
    }));
    assert_eq!(
        h.next().plugin_parameters.unwrap().source,
        Some(original_binding)
    );
    assert!((0..8).any(|_| {
        render_completed_block(h)
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.001)
    }));
    assert!(h.degraded_devices.is_empty());
}
