//! A real admitted worker read failure reaches the current callback demand.
use super::*;

#[test]
fn current_read_failure_notifies_without_partial_audio_or_identity_fault() {
    let _serial = serial();
    assert!(hex_engine::audio_storage_constructor_layout_supported());
    let project = readiness::project_with_frames(8192);
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(8192);
    let definition = wav.definition(asset, 11);
    let path = definition.path.clone();
    let original = std::fs::read(&path).unwrap();
    let key = hex_stream::PageKey {
        source: definition.key,
        page_index: 1,
    };
    let (gate, entered, release) = hex_stream::test_support::PageGate::new(key);
    let mut owner = stream_fixture::owner_with_context(
        &project,
        definition.metadata.clone(),
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 9,
        }),
    );
    let mut pages = hex_stream::StreamRuntime::new_with_page_gate(
        hex_stream::StreamConfig::with_pool_pages(4),
        vec![definition],
        gate.with_read_failure_after(0),
    )
    .unwrap();
    pages
        .wait_until_ready(std::time::Duration::from_secs(1))
        .unwrap();
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(3).unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            pages.split().ok().unwrap(),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .ok()
        .unwrap();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut render = |output: &mut [[f32; 2]]| {
        let count = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(output).unwrap();
            })
        });
        assert_eq!(
            (
                count.count_total,
                count.bytes_total,
                count.count_current,
                count.bytes_current
            ),
            (0, 0, 0, 0)
        );
    };
    let mut output = [[0.0; 2]; 256];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while control.status.applied_revision.load(Acquire) == 0 {
        render(&mut output);
        assert!(!control.status.take_stream_read_failure());
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(output, [[0.25; 2]; 256]);
    while control.status.position.load(Acquire) < 4096 {
        render(&mut output);
        assert_eq!(output, [[0.25; 2]; 256]);
        assert!(!control.status.take_stream_read_failure());
        assert!(std::time::Instant::now() < deadline);
    }
    entered
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    release.send(()).unwrap();
    while !control.status.stream_readiness_refused.load(Acquire) {
        render(&mut output);
        assert_eq!(output, [[0.0; 2]; 256]);
        assert_eq!(control.status.position.load(Acquire), 4096);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    let reported = control.status.stream_read_failed.load(Acquire);
    let identity_unchanged = !control.status.stream_source_changed.load(Acquire);
    let no_render_fault = !control.status.render_fault.load(Acquire);
    control.stop();
    render(&mut output);
    let stopped = !control.status.playing.load(Acquire);
    let retained_after_stop = control.status.take_stream_read_failure();
    let consumed = !control.status.take_stream_read_failure();
    control.command(PlaybackCommand::Play).unwrap();
    while control.status.position.load(Acquire) <= 4096 {
        render(&mut output);
        assert!(
            output
                .iter()
                .all(|frame| *frame == [0.25; 2] || *frame == [0.0; 2])
        );
        assert!(!control.status.take_stream_read_failure());
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(output, [[0.25; 2]; 256], "verified retry restores real PCM");
    control.stop();
    render(&mut output);
    let source_unchanged = std::fs::read(&path).unwrap() == original;
    let mut backend = Some(());
    let mut pending = false;
    let mut disposed = false;
    crate::lifecycle::test_dispose_backend(
        &mut backend,
        &mut pending,
        &mut disposed,
        &mut retirement,
        |_| Ok(()),
    )
    .unwrap();
    drop(data);
    drop(errors);
    retirement.advance().unwrap();
    let returned = recover(&mut control, &ticket);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let report = loop {
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            break report;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    };
    drop(returned);
    drop(control);
    finish(&mut retirement);
    assert!(report.worker_joined && report.accounting_complete);
    assert_eq!(report.page_buffers, 4);
    assert!(reported, "current generic read failure must notify control");
    assert!(
        consumed
            && retained_after_stop
            && identity_unchanged
            && no_render_fault
            && stopped
            && source_unchanged
    );
}
