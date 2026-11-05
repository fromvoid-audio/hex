use super::first_load::{authored, request};
use super::*;
use crate::source_overview::OverviewBudget;

#[test]
fn actual_worker_old_view_survives_new_open_remains_charged_until_last_reference_and_retry() {
    let files = Files::new(true);
    let standard = OverviewBudget::standard().unwrap().usage();
    let budget = OverviewBudget::with_limits(4 * 16, standard.max_metadata_bytes).unwrap();
    let baseline = budget.usage();
    let service = Service::start_configured_in_overview_domain(
        16000,
        16,
        1024,
        None,
        canonical_exporter(),
        None,
        Some(reservation()),
        budget.clone(),
    )
    .unwrap();
    let (track, before) = authored(&service);
    service
        .submit(request(&before, track, files.source.clone()))
        .unwrap();
    let loaded = event(&service);
    assert!(loaded.error.is_none(), "{:?}", loaded.error);
    let asset = loaded.state.project.assets().next().unwrap().id();
    assert!(
        loaded
            .state
            .project
            .asset(asset)
            .unwrap()
            .original_source()
            .is_none()
    );
    let overview = loaded.state.source_overviews.get(asset).unwrap();
    assert_eq!(
        (
            overview.source_frames(),
            overview.sample_rate(),
            overview.bin_count()
        ),
        (4, 8000, 4)
    );
    let expected = [[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]];
    assert_eq!(
        overview.peaks().copied().collect::<Vec<_>>(),
        expected
            .map(|f| f.map(|s| hex_media::waveform::Peak { min: s, max: s }))
            .to_vec()
    );
    let content = overview.content();
    let retained = Arc::clone(&loaded.state);
    let changed = edit(&service, Command::Gain { track, gain: 0.5 });
    assert!(
        changed
            .state
            .source_overviews
            .shares_storage_with(&loaded.state.source_overviews)
    );
    assert!(
        changed
            .state
            .source_overviews
            .matches_context(hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: changed.state.revision,
            })
    );
    assert!(
        !loaded
            .state
            .source_overviews
            .matches_context(hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: changed.state.revision,
            })
    );
    let saved = files.source.parent().unwrap().join("overview.hex");
    service.submit(Request::SaveNew(saved.clone())).unwrap();
    let saved_event = event(&service);
    assert!(saved_event.error.is_none(), "{:?}", saved_event.error);
    assert!(
        saved_event
            .state
            .source_overviews
            .shares_storage_with(&loaded.state.source_overviews)
    );
    assert_eq!(
        hex_storage::load(&saved)
            .unwrap()
            .asset(asset)
            .unwrap()
            .original_source(),
        Some(content)
    );
    service
        .submit(Request::NewWithAudioMode {
            mode: crate::DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    let empty = event(&service);
    assert!(empty.error.is_none(), "{:?}", empty.error);
    assert!(empty.state.source_overviews.is_empty());
    assert_eq!(
        budget.usage().bin_capacity_bytes,
        4 * 16,
        "old view retains peak backing after actual New clears Session"
    );
    service.submit(Request::Open(saved.clone())).unwrap();
    let refused = event(&service);
    assert!(
        refused
            .error
            .as_ref()
            .unwrap()
            .contains("overview peak budget")
    );
    assert_eq!(
        refused.state.project.snapshot(),
        empty.state.project.snapshot()
    );
    assert_eq!(refused.state.revision, empty.state.revision);
    assert!(
        refused
            .state
            .source_overviews
            .shares_storage_with(&empty.state.source_overviews)
    );
    assert_eq!(budget.usage().bin_capacity_bytes, 4 * 16);
    drop(saved_event);
    drop(changed);
    drop(loaded);
    assert_eq!(
        budget.usage().bin_capacity_bytes,
        4 * 16,
        "last external old view still owns the same backing"
    );
    drop(retained);
    assert_eq!(budget.usage(), baseline);
    service.submit(Request::Open(saved)).unwrap();
    let reopened = event(&service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    let overview = reopened.state.source_overviews.get(asset).unwrap();
    assert_eq!(overview.content(), content);
    assert_eq!(
        overview.peaks().copied().collect::<Vec<_>>(),
        expected
            .map(|f| f.map(|s| hex_media::waveform::Peak { min: s, max: s }))
            .to_vec()
    );
    assert_eq!(budget.usage().bin_capacity_bytes, 4 * 16);
}
