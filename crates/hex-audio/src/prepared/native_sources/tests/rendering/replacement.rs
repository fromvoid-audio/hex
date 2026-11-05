//! Original native replacement at final graph acceptance. Routed latency
//! changes retain explicit refusal until their separate compensation contract.
use super::*;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_replacement_selects_new_source_or_preserves_original_on_refusal() {
    let _serial = serial();
    for location in 0..3 {
        let PreparedFixture {
            mut control,
            mut data,
            errors,
            retirement,
            binding,
            unused,
            owner,
            project,
            samples,
        } = prepared_fixture_with_profile(location, false, "processing", 8, false);
        assert!(unused.is_none());
        let root = control.native_candidate_root();
        let first = control
            .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 41)
            .unwrap_or_else(|r| panic!("first graph: {}", r.reason));
        control.command(PlaybackCommand::Play).unwrap();
        let mut successor = None;
        let mut reintroduced = None;
        let mut output = [[0.0; 2]; 64];
        for (block, chunk) in output.chunks_mut(8).enumerate() {
            if block == 1 {
                let (owner, device) = match location {
                    0 => (
                        ProcessorChainOwner::Track(project.tracks()[0].id().value()),
                        project.tracks()[0].devices()[0].id(),
                    ),
                    1 => (
                        ProcessorChainOwner::Return(project.return_tracks()[0].id().value()),
                        project.return_tracks()[0].devices()[0].id(),
                    ),
                    _ => (ProcessorChainOwner::Main, project.main_devices()[0].id()),
                };
                let saved = control.native_source_prepared_state(binding).unwrap();
                let state = hex_plugin_state::decode(&saved).unwrap();
                let mut component = state.component.unwrap().to_vec();
                // Supported original fixture component state: HXPS, latency,
                // then linear gain. Actual startup restores/captures these bytes.
                component[8..12].copy_from_slice(&0.75_f32.to_le_bytes());
                let restored = hex_plugin_state::encode(hex_plugin_state::State {
                    component: Some(&component),
                    ..state
                })
                .unwrap();
                let next = source_for_profile(
                    &mut control,
                    ProcessorInstanceIdentity::new(owner, device.value()).unwrap(),
                    "processing",
                    8,
                    Some(restored),
                );
                assert_ne!(next, binding);
                let captured = control.native_source_prepared_state(next).unwrap();
                assert_ne!(captured, saved);
                let mut editor = Editor::new(project.clone(), 8).unwrap();
                editor
                    .execute(Command::ReplaceDeviceState {
                        device,
                        state: captured,
                    })
                    .unwrap();
                control.publish_native_source_endpoint(next).unwrap();
                allocation_free(|| {
                    data.owner_mut()
                        .unwrap()
                        .run(|callback, _| callback.begin_block())
                });
                successor = Some((
                    next,
                    replacement_preparation(&mut control, &[next], editor.project(), &samples),
                ));
            }
            if block == 2 && location == 2 {
                // The original A is still processing its retained tail while
                // current B runs. A new graph cannot reintroduce that exact A
                // endpoint as current, even though it is a live Root source.
                reintroduced = Some(replacement_preparation(
                    &mut control,
                    &[binding],
                    &project,
                    &samples,
                ));
            }
            let mut host = [0.0_f32; 16];
            allocation_free(|| {
                data.owner_mut().unwrap().run(|callback, _| {
                    callback.begin_block();
                    callback.begin_native_effect_callback(8).unwrap();
                    let rendered = callback.render(chunk);
                    assert!(rendered.is_ok(), "owner {location}: {rendered:?}");
                    assert!(crate::device::finish_host_chunk(
                        callback,
                        &mut host,
                        chunk,
                        2,
                        &rendered,
                        block as u64 * 8,
                        Instant::now()
                    ));
                })
            });
            assert_eq!(
                control.status.applied_revision.load(Acquire),
                if location == 2 && block > 0 { 2 } else { 1 }
            );
            let expected_old = if location == 2 {
                (block as u64 + 1).min(4)
            } else {
                block as u64 + 1
            };
            wait_for_blocks(&root, binding, expected_old);
            if let Some((next, _)) = successor.as_ref() {
                if location == 2 {
                    wait_for_blocks(&root, *next, block as u64);
                } else {
                    assert_eq!(
                        crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(&root, *next),
                        Some(0)
                    );
                }
            }
            if block == 1 && location != 2 {
                let refused = crate::prepared::candidate_tests::recover(
                    &mut control,
                    &successor.as_ref().unwrap().1,
                );
                assert_eq!(
                    refused.return_kind,
                    crate::AudioCandidateReturnKind::NeverInstalled
                );
                drop(refused);
            }
            if block == 2 && location == 2 {
                let refused = crate::prepared::candidate_tests::recover(
                    &mut control,
                    reintroduced.as_ref().unwrap(),
                );
                assert_eq!(
                    refused.return_kind,
                    crate::AudioCandidateReturnKind::NeverInstalled
                );
                drop(refused);
            }
        }
        let mut expected = [[0.0; 2]; 64];
        for (index, frame) in samples.iter().enumerate() {
            let gain = if location == 2 {
                if index < 8 {
                    0.5
                } else {
                    (0.5 + ((index - 8) / 8) as f32 * 0.125) * 0.75
                }
            } else {
                0.5 + (index / 8) as f32 * 0.125 + if location == 1 { 1.0 } else { 0.0 }
            };
            expected[index + 16] = [frame[0] * gain, frame[1] * gain];
        }
        assert_eq!(
            output, expected,
            "source-selected replacement owner {location}"
        );
        let mut retirement = close_backend(data, errors, retirement);
        if location == 2 {
            let old = crate::prepared::candidate_tests::recover(&mut control, &first);
            assert_eq!(old.return_kind, crate::AudioCandidateReturnKind::Replaced);
            drop(old);
        }
        let (next, second) = successor.unwrap();
        let current = crate::prepared::candidate_tests::recover(
            &mut control,
            if location == 2 { &second } else { &first },
        );
        assert_eq!(
            current.return_kind,
            crate::AudioCandidateReturnKind::FinalActive
        );
        drop(current);
        for (source, blocks) in [
            (binding, if location == 2 { 4 } else { 8 }),
            (next, if location == 2 { 7 } else { 0 }),
        ] {
            let returned = recover(&mut control, source);
            assert!(returned.error.is_none() && returned.cleanup_error.is_none());
            let native::PreparationDisposal::Reaped(disposal) = returned.disposal else {
                panic!("original native disposal")
            };
            assert!(disposal.status.success());
            let terminal = disposal.terminal.unwrap();
            assert_eq!((terminal.blocks, terminal.frames), (blocks, blocks * 8));
        }
        drop(control);
        finish(&mut retirement);
    }
}

pub(super) fn wait_for_blocks(
    root: &crate::candidate_custody::CandidateRoot,
    binding: Binding,
    blocks: u64,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(
        root, binding,
    )
    .unwrap()
        < blocks
    {
        assert!(
            Instant::now() < deadline,
            "original native worker completion"
        );
        std::thread::yield_now();
    }
}
