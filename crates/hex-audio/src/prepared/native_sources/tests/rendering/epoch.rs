//! Actual stopped output/source return and same-instance successor rendering.
use super::*;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_stop_returns_graph_and_endpoint_before_same_instance_successor_pcm() {
    let _serial = serial();
    for (location, source_count) in (0..3).flat_map(|location| (1..=2).map(move |n| (location, n)))
    {
        let PreparedFixture {
            mut control,
            mut data,
            errors,
            retirement,
            binding,
            mut owner,
            mut project,
            samples,
            ..
        } = prepared_fixture_with_profile(location, false, "processing", 8, false);
        // Use the actual current-source preparation path, including its source
        // context and transport fence. The unsubmitted constructor is disposed
        // on control, without ever acquiring a Root publication/source reference.
        owner.dispose_on_control().unwrap();
        owner.dispose_original_inputs_on_control().unwrap();
        drop(owner);
        let mut bindings = vec![binding];
        if source_count == 2 {
            let owner = match location {
                0 => ProcessorChainOwner::Track(project.tracks()[0].id().value()),
                1 => ProcessorChainOwner::Return(project.return_tracks()[0].id().value()),
                _ => ProcessorChainOwner::Main,
            };
            let second = source_for_profile(
                &mut control,
                ProcessorInstanceIdentity::new(owner, project.next_identity()).unwrap(),
                "processing",
                8,
                None,
            );
            let state = control.native_source_prepared_state(second).unwrap();
            let command = match location {
                0 => Command::InsertDevice {
                    track: project.tracks()[0].id(),
                    before: None,
                    state,
                },
                1 => Command::InsertReturnDevice {
                    return_track: project.return_tracks()[0].id(),
                    before: None,
                    state,
                },
                _ => Command::InsertMainDevice {
                    before: None,
                    state,
                },
            };
            let mut editor = Editor::new(project, 8).unwrap();
            editor.execute(command).unwrap();
            project = editor.project().clone();
            control.publish_native_source_endpoint(second).unwrap();
            allocation_free(|| {
                data.owner_mut()
                    .unwrap()
                    .run(|callback, _| callback.begin_block())
            });
            bindings.push(second);
        }
        let states: Vec<_> = bindings
            .iter()
            .map(|binding| control.native_source_prepared_state(*binding).unwrap())
            .collect();
        let mut ticket = replacement_preparation(&mut control, &bindings, &project, &samples);
        let root = control.native_candidate_root();
        let mut previous_ticket = None;
        let blocks_per_epoch = 2 + source_count as u64 * 2;
        for epoch in 0..3_u64 {
            // Installation and each real host-format acceptance remain separate
            // from actual worker completion; no wall-clock audio timing oracle.
            control.command(PlaybackCommand::Play).unwrap();
            let mut output = vec![[0.0; 2]; blocks_per_epoch as usize * 8];
            for (n, chunk) in output.chunks_mut(8).enumerate() {
                let mut host = [0.0_f32; 16];
                allocation_free(|| {
                    data.owner_mut().unwrap().run(|callback, _| {
                        callback.begin_block();
                        callback.begin_native_effect_callback(8).unwrap();
                        let rendered = callback.render(chunk);
                        assert!(rendered.is_ok(), "epoch{epoch} block{n}: {rendered:?}");
                        assert!(crate::device::finish_host_chunk(
                            callback,
                            &mut host,
                            chunk,
                            2,
                            &rendered,
                            (epoch * blocks_per_epoch + n as u64) * 8,
                            Instant::now()
                        ));
                    })
                });
                for binding in &bindings {
                    replacement::wait_for_blocks(
                        &root,
                        *binding,
                        epoch * blocks_per_epoch + n as u64 + 1,
                    );
                }
            }
            let mut expected = vec![[0.0; 2]; output.len()];
            for index in 0..16 {
                let mut gain = 1.0;
                for source_index in 0..source_count {
                    gain *= 0.5
                        + (epoch * blocks_per_epoch + (source_index * 2 + index / 8) as u64) as f32
                            * 0.125;
                }
                if location == 1 {
                    gain += 1.0;
                }
                expected[index + source_count * 16] =
                    [samples[index][0] * gain, samples[index][1] * gain];
            }
            assert_eq!(
                output, expected,
                "location{location} sources{source_count} epoch{epoch}: no old queued replies or DSP reset"
            );
            if epoch == 2 {
                break;
            }
            let expected_source = control
                .status
                .applied_audio_source()
                .expect("actual current source");
            control.stop();
            let fence = control.request_native_epoch_fence(expected_source).unwrap();
            allocation_free(|| {
                data.owner_mut()
                    .unwrap()
                    .run(|callback, _| callback.begin_block())
            });
            assert_eq!(
                control.stopped_edit_fence_state(fence),
                StoppedEditFenceState::Ready
            );
            assert!(control.command(PlaybackCommand::Play).is_err());
            assert!(control.status.reserve_midi_begin().is_err());
            assert!(!control.status.publication_pending.reserve_publication());
            data.owner_mut().unwrap().run(|callback, _| {
                let Some(ActivePublication::Eligible(active)) = callback.active.as_ref() else {
                    panic!("original active runtime");
                };
                let target = ticket.native_epoch_target(root.root_id()).unwrap();
                assert!(active.matches_native_epoch_target(&target));
                assert!(
                    callback
                        .status
                        .sampler_edit
                        .matches_source(fence, active.applied_audio_source().unwrap())
                );
                assert!(!active.runtime().unwrap().status().unwrap().playing);
                assert!(callback.native_sources.has_endpoints());
            });
            if let Some(stale) = previous_ticket.as_ref() {
                control
                    .return_stopped_native_epoch_runtime(stale, fence)
                    .unwrap();
                allocation_free(|| {
                    data.owner_mut()
                        .unwrap()
                        .run(|callback, _| callback.begin_block())
                });
                assert_eq!(
                    control.status.applied_audio_source(),
                    Some(expected_source),
                    "an old runtime ticket cannot return its successor"
                );
            }
            control
                .return_stopped_native_epoch_runtime(&ticket, fence)
                .unwrap();
            assert!(
                control
                    .quiesce_native_source_preparation(bindings[0], bindings[0].graph)
                    .is_err()
            );
            allocation_free(|| {
                data.owner_mut()
                    .unwrap()
                    .run(|callback, _| callback.begin_block())
            });
            assert!(
                control
                    .quiesce_native_source_preparation(bindings[0], bindings[0].graph)
                    .is_err(),
                "enqueue/callback return is not control ingestion"
            );
            assert_eq!(
                control.status.applied_audio_source(),
                None,
                "exact stopped runtime must return at its callback frontier"
            );
            // With two sources, the actual returned runtime plus the first
            // endpoint fill the bounded return ring. The second stays on callback
            // until control drains it; independent Stop must still be consumed.
            allocation_free(|| {
                data.owner_mut()
                    .unwrap()
                    .run(|callback, _| callback.begin_block())
            });
            if source_count == 2 {
                data.owner_mut().unwrap().run(|callback, _| {
                    assert!(callback.retired.is_full());
                    assert!(callback.native_sources.has_endpoints());
                });
                control.stop();
                allocation_free(|| {
                    data.owner_mut()
                        .unwrap()
                        .run(|callback, _| callback.begin_block())
                });
                assert_eq!(control.status.pending_stop_marker.load(Acquire), 0);
                assert!(!control.status.playing.load(Acquire));
                for binding in &bindings {
                    assert!(
                        control
                            .quiesce_native_source_preparation(*binding, binding.graph)
                            .is_err()
                    );
                }
            }
            let old = crate::prepared::candidate_tests::recover(&mut control, &ticket);
            assert_eq!(
                old.return_kind,
                crate::AudioCandidateReturnKind::StoppedForNativeEpoch
            );
            drop(old);
            allocation_free(|| {
                data.owner_mut()
                    .unwrap()
                    .run(|callback, _| callback.begin_block())
            });
            control.reclaim();
            for (binding, state) in bindings.iter_mut().zip(&states) {
                let deadline = Instant::now() + Duration::from_secs(3);
                while crate::candidate_custody::native_source_test_fixture::original_worker_awaits_endpoint(&root, *binding) {
                    control.reclaim();
                    assert!(Instant::now() < deadline, "actual old endpoint return");
                    std::thread::yield_now();
                }
                let old_loan = loop {
                    match control.publish_native_source_endpoint(*binding) {
                        Err("native preparation is busy") => {
                            assert!(Instant::now() < deadline, "old pair control lane");
                            std::thread::yield_now();
                        }
                        result => break result,
                    }
                };
                assert_eq!(
                    old_loan,
                    Err("native effect pair is unavailable"),
                    "a revoked returned endpoint requires a new native epoch ACK before publication"
                );
                let next = renew_after_returns(&mut control, *binding);
                assert!(next.epoch > binding.epoch);
                assert_eq!(
                    (next.root, next.instance, next.graph),
                    (binding.root, binding.instance, binding.graph)
                );
                assert!(control.native_source_prepared_state(next).is_none());
                assert!(control.publish_native_source_endpoint(next).is_err());
                on_control(|| {
                    control.complete_native_source_preparation(
                        next,
                        Some(hex_plugin_host::protocol::PresentationLatency {
                            input_frames: 77,
                            output_frames: 91,
                        }),
                    )
                });

                assert_eq!(
                    control.native_source_prepared_state(next).as_ref(),
                    Some(state)
                );
                on_control(|| control.start_native_effect(next));
                on_control(|| control.publish_native_source_endpoint(next));
                allocation_free(|| {
                    data.owner_mut()
                        .unwrap()
                        .run(|callback, _| callback.begin_block())
                });
                *binding = next;
            }
            allocation_free(|| {
                data.owner_mut()
                    .unwrap()
                    .run(|callback, _| callback.begin_block())
            });
            previous_ticket = Some(ticket);
            ticket =
                replacement_epoch_preparation(&mut control, &bindings, &project, &samples, fence);
            allocation_free(|| {
                data.owner_mut()
                    .unwrap()
                    .run(|callback, _| callback.begin_block())
            });
            assert_eq!(
                control.status.applied_audio_source().unwrap().context,
                expected_source.context
            );
            control.release_stopped_edit_fence(fence);
            assert!(
                control
                    .return_stopped_native_epoch_runtime(&ticket, fence)
                    .is_err(),
                "released native epoch token cannot return a new runtime"
            );
        }
        let mut retirement = close_backend(data, errors, retirement);
        drop(crate::prepared::candidate_tests::recover(
            &mut control,
            &ticket,
        ));
        for binding in bindings {
            let returned = recover(&mut control, binding);
            assert!(returned.error.is_none() && returned.cleanup_error.is_none());
            let native::PreparationDisposal::Reaped(retired) = returned.disposal else {
                panic!("actual native disposal")
            };
            assert!(retired.status.success());
            let terminal = retired.terminal.unwrap();
            assert_eq!(
                (terminal.blocks, terminal.frames),
                (3 * blocks_per_epoch, 3 * blocks_per_epoch * 8),
                "same instance cumulative prefix"
            );
        }
        drop(control);
        finish(&mut retirement);
    }
}

// The production control APIs explicitly refuse a busy Work lane before taking
// an owner. Drive those actual retryable refusals, never retry an invalid phase,
// manufacture completion or delay the callback to hide an output mismatch.
pub(super) fn on_control<T>(mut operation: impl FnMut() -> Result<T, &'static str>) -> T {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match operation() {
            Ok(result) => return result,
            Err(
                reason @ ("native preparation is busy"
                | "native effect preparation is busy"
                | "native effect selection is busy"
                | "candidate preparation is busy"),
            ) => {
                assert!(
                    Instant::now() < deadline,
                    "control lane stayed busy: {reason}"
                );
                std::thread::yield_now();
            }
            Err(reason) => panic!("actual control operation refused: {reason}"),
        }
    }
}

pub(super) fn renew_after_returns(control: &mut Control, binding: Binding) -> Binding {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        // Endpoint ingestion may be refused while the existing lifecycle worker
        // owns Work. Reclaim retains the exact original return for another poll.
        control.reclaim();
        match control.quiesce_native_source_preparation(binding, binding.graph) {
            Ok(next) => return next,
            Err(
                reason @ ("native preparation is busy"
                | "native epoch requires original graph and endpoint return"),
            ) => {
                assert!(
                    Instant::now() < deadline,
                    "old owners did not return: {reason}"
                );
                std::thread::yield_now();
            }
            Err(reason) => panic!("native epoch refused: {reason}"),
        }
    }
}
