//! Actual page-pinned native Track and Return playback across rooted successors.
use super::*;
use hex_engine::MediaPages;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn closed_streamed_track_worker_survives_cancelled_and_applied_successors() {
    verify_closed_successors(0, 0.0);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn closed_streamed_return_worker_survives_cancelled_and_applied_successors() {
    verify_closed_successors(1, 1.0);
}

fn verify_closed_successors(location: usize, dry_mix: f32) {
    let _serial = serial();
    let PreparedFixture {
        mut control,
        mut data,
        errors,
        retirement,
        binding,
        mut owner,
        project,
        samples,
        ..
    } = prepared_fixture_with_profile(location, false, "processing", 8, false);
    owner.dispose_on_control().unwrap();
    owner.dispose_original_inputs_on_control().unwrap();
    drop(owner);
    control.configure_audio_candidate_streaming(3).unwrap();
    let mut editor = Editor::new(project, 8).unwrap();
    let asset = editor.project().assets().next().unwrap().id();
    let track = editor.project().tracks()[0].id();
    let wav = crate::prepared::eligible_publication::tests::stream_fixture::Wav::with_frames(
        samples.len() as u32,
    );
    let definition = wav.definition(asset, 12);
    let metadata = definition.metadata.clone();
    let key = metadata.key();
    let mut stream = hex_stream::StreamRuntime::new(
        hex_stream::StreamConfig::with_pool_pages(4),
        vec![definition],
    )
    .unwrap();
    stream.wait_until_ready(Duration::from_secs(2)).unwrap();
    let (stream_control, mut pages) = stream.split().ok().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    pages.request_page(key, 0);
    while pages.page(key, 0).is_none() {
        pages.pump_ready(2);
        assert!(Instant::now() < deadline, "verified page readiness");
        std::thread::yield_now();
    }
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            (stream_control, pages),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    allocation_free(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    let stage = |control: &mut Control, project: &Project| {
        replacement_preparation_inner(
            control,
            &[binding],
            project,
            &[],
            None,
            vec![domain],
            Some(metadata.clone()),
        )
    };
    let first = stage(&mut control, editor.project());
    let root = control.native_candidate_root();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 64];
    let mut cancelled = None;
    let mut successor = None;
    for (block, chunk) in output.chunks_mut(8).enumerate() {
        if matches!(block, 2 | 4) {
            editor
                .execute(Command::RenameTrack {
                    track,
                    name: format!("source {block}"),
                })
                .unwrap();
            let ticket = stage(&mut control, editor.project());
            if block == 2 {
                assert_eq!(
                    control.cancel_audio_candidate(&ticket),
                    crate::AudioCandidateCancellationResult::Cancelled
                );
                cancelled = Some(ticket);
            } else {
                successor = Some(ticket);
            }
        }
        allocation_free(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.begin_native_effect_callback(8).unwrap();
                let rendered = callback.render(chunk);
                assert!(rendered.is_ok(), "{rendered:?}");
                assert!(crate::device::finish_host_chunk(
                    callback,
                    &mut [0.0; 16],
                    chunk,
                    2,
                    &rendered,
                    block as u64 * 8,
                    Instant::now(),
                ));
            });
        });
        let revision = if successor.is_some() { 3 } else { 1 };
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert_eq!(
            control.status.performance_frame.load(Acquire),
            (block as u64 + 1) * 8
        );
        replacement::wait_for_blocks(&root, binding, block as u64 + 1);
        if let Some(ticket) = cancelled.take() {
            let returned = crate::prepared::candidate_tests::recover(&mut control, &ticket);
            assert_eq!(
                returned.return_kind,
                crate::AudioCandidateReturnKind::NeverInstalled
            );
        }
    }
    let mut expected = [[0.0; 2]; 64];
    for frame in 0..samples.len() {
        let wet_gain = 0.5 + (frame / 8) as f32 * 0.125;
        expected[frame + 16] = [0.25 * (dry_mix + wet_gain); 2];
    }
    assert_eq!(output, expected);

    let mut retirement = close_backend(data, errors, retirement);
    let initial = crate::prepared::candidate_tests::recover(&mut control, &first);
    assert_eq!(
        initial.return_kind,
        crate::AudioCandidateReturnKind::Replaced
    );
    drop(initial);
    let final_active = crate::prepared::candidate_tests::recover(&mut control, &successor.unwrap());
    assert_eq!(
        final_active.return_kind,
        crate::AudioCandidateReturnKind::FinalActive
    );
    drop(final_active);
    let returned = recover(&mut control, binding);
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
        panic!("exact source disposal");
    };
    assert!(disposal.status.success());
    assert_eq!(disposal.terminal.unwrap().blocks, 8);
    drop(control);
    finish(&mut retirement);
}
