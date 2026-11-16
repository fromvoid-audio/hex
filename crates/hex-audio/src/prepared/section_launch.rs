//! One scheduled seek in the existing fresh-publication slot. No new queue,
//! graph owner or callback destructor is introduced. Preparation readiness
//! selects the next boundary on the old graph; the callback renders its prefix,
//! installs the exact fresh candidate, then renders its suffix. Stop/cancel and
//! the source/route/command fence remain effective while the candidate waits.
use super::*;

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod midi_tests;

pub(super) enum QuantizedSeekRender {
    NotHandled,
    Unchanged,
    Switched,
}

impl Callback {
    pub(super) fn quantized_seek_publication_due(&mut self) -> bool {
        let Some(next) = self.deferred.as_mut() else {
            return true;
        };
        let PublicationPolicy::InstallQuantizedSeekWhenReady {
            fence,
            quantization,
        } = next.policy
        else {
            return true;
        };
        let old = self.active.as_ref().and_then(ActivePublication::legacy);
        let cancelled = next
            .fresh_runtime_cancellation
            .as_ref()
            .is_some_and(|cell| cell.decision.load(Acquire) == FRESH_PUBLICATION_CANCELLED);
        let stale = old.is_none_or(|old| {
            old.revision != fence.expected_active_revision
                || old.playback.session_route_state_generation() != fence.route_state_generation
        }) || self.status.admitted_ticket.load(Acquire)
            != fence.admitted_command_ticket
            || self.status.last_output_fault_epoch.load(Acquire) != fence.output_fault_epoch
            || self.status.consumed_stop_generation.load(Acquire) != fence.consumed_stop_generation
            || self.status.render_fault.load(Acquire)
            || self.status.device_fault.load(Acquire)
            || self.status.pending_stop_marker.load(Acquire) != 0;
        let result = if cancelled {
            Err(PlaybackHandoffRejection::FreshRuntimeCancelled)
        } else if stale {
            Err(PlaybackHandoffRejection::RuntimeFenceChanged)
        } else if let Some(old) = old {
            let current = old.playback.performance_frame();
            if next.quantized_seek_frame.is_none_or(|due| due < current) {
                match old.playback.quantized_seek_frame(quantization) {
                    Ok(due) => next.quantized_seek_frame = Some(due),
                    Err(_) => {
                        return self.reject_quantized_seek_publication(
                            PlaybackHandoffRejection::SessionRuntime,
                        );
                    }
                }
            }
            Ok(next.quantized_seek_frame == Some(current))
        } else {
            Err(PlaybackHandoffRejection::RuntimeFenceChanged)
        };
        match result {
            Ok(due) => due,
            Err(reason) => self.reject_quantized_seek_publication(reason),
        }
    }

    fn reject_quantized_seek_publication(&mut self, reason: PlaybackHandoffRejection) -> bool {
        if let Some(mut next) = self.deferred.take() {
            if let Some(streams) = next.streams.as_mut() {
                streams.cancel_install(&mut self.domain_pages);
            }
            if let Some(cell) = next.fresh_runtime_cancellation.as_ref() {
                cell.finish();
            }
            self.status.publish_rejected_revision(next.revision, reason);
            self.return_graph(next);
            self.status.publication_pending.store(false, Release);
        }
        false
    }

    pub(super) fn render_quantized_seek(
        &mut self,
        output: &mut [Frame],
    ) -> Result<QuantizedSeekRender, Invalid> {
        let Some(due) = self
            .deferred
            .as_ref()
            .filter(|next| {
                matches!(
                    next.policy,
                    PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
                )
            })
            .and_then(|next| next.quantized_seek_frame)
        else {
            return Ok(QuantizedSeekRender::NotHandled);
        };
        let Some(old) = self.active.as_ref().and_then(ActivePublication::legacy) else {
            return Ok(QuantizedSeekRender::NotHandled);
        };
        if !old.playback.playing() {
            return Ok(QuantizedSeekRender::NotHandled);
        }
        let Some(distance) = due.checked_sub(old.playback.performance_frame()) else {
            return Ok(QuantizedSeekRender::NotHandled);
        };
        let Ok(prefix) = usize::try_from(distance) else {
            return Ok(QuantizedSeekRender::NotHandled);
        };
        if prefix > output.len() {
            return Ok(QuantizedSeekRender::NotHandled);
        }
        let old_revision = old.revision;
        let (before, after) = output.split_at_mut(prefix);
        self.render_project(before, 0)?;
        // Revalidate immediately before the swap, including a cancellation or
        // priority Stop racing the old prefix. Retired owners keep ordinary
        // fixed return-ring custody, including under backpressure.
        self.advance_legacy_publication(Some(after.len()));
        let switched = self
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .is_some_and(|active| active.revision != old_revision);
        let suffix_start = self.scheduled_transport();
        self.render_project(after, prefix)?;
        if switched {
            let played = match (suffix_start, self.scheduled_transport()) {
                (Some((_, start, true)), Some((_, end, _))) => {
                    usize::try_from(end.saturating_sub(start))
                        .map_or(after.len(), |frames| frames.min(after.len()))
                }
                _ => 0,
            };
            self.record_audible_run(suffix_start, played);
            Ok(QuantizedSeekRender::Switched)
        } else {
            Ok(QuantizedSeekRender::Unchanged)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::{Audio, LaunchQuantization, PreparedRevision, SessionPlayback, SineTable};
    use hex_project::{Command, Created, Editor, LoopRegion, Project};
    use std::collections::BTreeMap;

    fn source() -> Arc<PreparedRevision> {
        source_with_latency(false)
    }

    fn source_with_latency(latency: bool) -> Arc<PreparedRevision> {
        let mut editor = Editor::new(Project::new(8_000).unwrap(), 8).unwrap();
        let Created::Imported { asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "original.wav".into(),
                sample_rate: 8_000,
                frames: 32_000,
            })
            .unwrap()
        else {
            panic!("import");
        };
        if latency {
            use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
            let definition = &hex_devices::limiter::DEFINITION;
            let limiter = definition
                .patch_parameter(
                    &definition.default_state().unwrap(),
                    hex_devices::limiter::parameter::LOOKAHEAD,
                    ParameterValue::Choice(2),
                )
                .unwrap();
            editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: limiter,
                })
                .unwrap();
        }
        let audio = Audio::new(
            8_000,
            (0..32_000)
                .map(|frame| [((frame / 4_000) + 1) as f32 / 8.0; 2])
                .collect(),
        )
        .unwrap();
        Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::from([(asset, audio)]),
                Arc::new(SineTable::new()),
                None,
            )
            .unwrap(),
        )
    }

    fn candidate(source: &Arc<PreparedRevision>) -> Playback {
        candidate_in_region(source, LoopRegion::new(16_000, 24_000, true).unwrap())
    }

    fn candidate_in_region(source: &Arc<PreparedRevision>, region: LoopRegion) -> Playback {
        Playback::from_interactive_arrangement_preroll(
            SessionPlayback::prepare_section_arrangement_preroll(
                Arc::clone(source),
                None,
                region,
                16_000,
                &AtomicBool::new(false),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn active(source: &Arc<PreparedRevision>, frame: u64, playing: bool) -> (Control, Callback) {
        let mut playback = Playback::try_from_prepared_with_session(Arc::clone(source)).unwrap();
        playback.apply(PlaybackCommand::Seek(frame));
        if playing {
            playback.apply(PlaybackCommand::Play);
        }
        let (mut control, mut callback) = channel(source.arrangement().sample_rate()).unwrap();
        control.publish(playback).unwrap();
        callback.begin_block();
        callback.render(&mut []).unwrap();
        (control, callback)
    }

    fn render_checked(callback: &mut Callback, output: &mut [Frame]) {
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(output).unwrap();
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            callback.begin_block();
            callback.render(output).unwrap();
        }
    }

    #[test]
    fn fault_rejects_waiting_section_instead_of_waiting_on_stopped_clock() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let source = source();
                let (mut control, mut callback) = active(&source, 3_997, true);
                let fence = control.capture_fresh_runtime_fence().unwrap();
                let revision = control
                    .publish_with_policy(
                        candidate(&source),
                        PublicationPolicy::InstallQuantizedSeekWhenReady {
                            fence,
                            quantization: LaunchQuantization::EightBars,
                        },
                    )
                    .unwrap();
                callback.report_render_fault(Some(&Invalid("injected failure")), 3_997);
                let mut output = [[1.0; 2]; 16];
                render_checked(&mut callback, &mut output);
                assert_eq!(output, [[0.0; 2]; 16]);
                assert_eq!(
                    control.status.rejected_revision(),
                    Some((revision, PlaybackHandoffRejection::RuntimeFenceChanged))
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn section_seek_captured_before_consumed_stop_is_refused() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let source = source();
                let (mut control, mut callback) = active(&source, 0, true);
                let fence = control.capture_fresh_runtime_fence().unwrap();
                control.stop();
                render_checked(&mut callback, &mut []);
                assert!(!control.status.playing.load(Acquire));
                let revision = control
                    .publish_with_policy(
                        candidate(&source),
                        PublicationPolicy::InstallQuantizedSeekWhenReady {
                            fence,
                            quantization: LaunchQuantization::Quarter,
                        },
                    )
                    .unwrap();
                let mut output = [[1.0; 2]; 8];
                render_checked(&mut callback, &mut output);
                assert_eq!(output, [[0.0; 2]; 8]);
                assert_eq!(control.status.applied_revision.load(Acquire), 1);
                assert_eq!(
                    control.status.rejected_revision(),
                    Some((revision, PlaybackHandoffRejection::RuntimeFenceChanged))
                );
                assert_eq!(control.reclaim(), 1);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn sample_recording_keeps_both_parts_of_a_section_launch() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let source = source();
                let (mut control, mut callback) = active(&source, 3_997, true);
                let generation = callback
                    .active
                    .as_ref()
                    .and_then(ActivePublication::current)
                    .unwrap()
                    .control_context_generation;
                let fence = control.capture_fresh_runtime_fence().unwrap();
                control
                    .publish_with_policy(
                        candidate(&source),
                        PublicationPolicy::InstallQuantizedSeekWhenReady {
                            fence,
                            quantization: LaunchQuantization::Quarter,
                        },
                    )
                    .unwrap();
                let mut take = control
                    .sample_recording
                    .start(generation, &[], true)
                    .unwrap();
                assert!(take.ready());
                let mut output = [[0.0; 2]; 17];
                render_checked(&mut callback, &mut output);
                callback.finish_sample_recording(true);
                let mut seen = false;
                assert_eq!(
                    take.drain(|frames, _, pcm| {
                        assert!(!seen);
                        assert_eq!(
                            crate::sample_recording::Reader::source_frames(pcm, frames, 0),
                            Some(&output[..])
                        );
                        seen = true;
                    }),
                    None
                );
                assert!(seen);
                assert_eq!(&output[..3], &[[0.125; 2]; 3]);
                assert_eq!(&output[3..], &[[0.625; 2]; 14]);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn quantized_section_once_ends_at_its_boundary_in_the_callback() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let source = source();
                for section_end in [None, Some(12_006)] {
                    let (mut control, mut callback) = active(&source, 3_997, true);
                    let fence = control.capture_fresh_runtime_fence().unwrap();
                    let mut candidate = Playback::from_interactive_arrangement_preroll(
                        SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                            Arc::clone(&source),
                            12_000,
                            0,
                            true,
                            &AtomicBool::new(false),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    if let Some(end) = section_end {
                        candidate
                            .arm_section_once(LoopRegion::new(12_000, end, false).unwrap())
                            .unwrap();
                    }
                    let revision = control
                        .publish_with_policy(
                            candidate,
                            PublicationPolicy::InstallQuantizedSeekWhenReady {
                                fence,
                                quantization: LaunchQuantization::Quarter,
                            },
                        )
                        .unwrap();

                    let mut output = [[0.0; 2]; 16];
                    render_checked(&mut callback, &mut output);
                    assert_eq!(&output[..3], &[[0.125; 2]; 3]);
                    if section_end.is_some() {
                        assert_eq!(&output[3..9], &[[0.5; 2]; 6]);
                        assert_eq!(&output[9..], &[[0.0; 2]; 7]);
                    } else {
                        assert_eq!(&output[3..], &[[0.5; 2]; 13]);
                    }
                    assert_eq!(control.status.applied_revision.load(Acquire), revision);
                    assert_eq!(
                        control.status.position.load(Acquire),
                        section_end.unwrap_or(12_013)
                    );
                    assert_eq!(control.status.playing.load(Acquire), section_end.is_none());
                    let active = callback
                        .active
                        .as_ref()
                        .and_then(ActivePublication::legacy)
                        .unwrap();
                    assert_eq!(active.playback.section_loop(), None);
                    assert_eq!(control.reclaim(), 1);
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn section_cut_tracks_audible_position_with_latency() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let source = source_with_latency(true);
                let latency = source.arrangement().main_latency_frames() as usize;
                assert!(latency > 14);
                for block in [1, 17, 257] {
                    let (mut control, mut callback) = active(&source, 3_997, true);
                    let fence = control.capture_fresh_runtime_fence().unwrap();
                    control
                        .publish_with_policy(
                            candidate_in_region(
                                &source,
                                LoopRegion::new(16_000, 16_064, true).unwrap(),
                            ),
                            PublicationPolicy::InstallQuantizedSeekWhenReady {
                                fence,
                                quantization: LaunchQuantization::Quarter,
                            },
                        )
                        .unwrap();
                    let mut before = [[0.0; 2]; 2];
                    for chunk in before.chunks_mut(block) {
                        render_checked(&mut callback, chunk);
                    }
                    assert_eq!(control.status.position.load(Relaxed), 3_999);
                    assert_eq!(control.status.audible_position.load(Relaxed), 3_997);
                    let mut first = [[0.0; 2]; 15];
                    for chunk in first.chunks_mut(block) {
                        render_checked(&mut callback, chunk);
                    }
                    assert_eq!(&first[1..], &[[0.0; 2]; 14], "block {block}");
                    assert_eq!(control.status.position.load(Relaxed), 16_014);
                    assert_eq!(
                        control.status.audible_position.load(Relaxed),
                        16_000,
                        "block {block}"
                    );
                    let mut rest = vec![[0.0; 2]; latency + 80 - 14];
                    for chunk in rest.chunks_mut(block) {
                        render_checked(&mut callback, chunk);
                    }
                    assert_eq!(
                        control.status.position.load(Relaxed),
                        16_000 + ((latency + 80) % 64) as u64,
                        "block {block}"
                    );
                    assert_eq!(
                        control.status.audible_position.load(Relaxed),
                        16_016,
                        "block {block}"
                    );
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn cancelled_section_cut_keeps_old_audible_history() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let source = source_with_latency(true);
                assert!(source.arrangement().main_latency_frames() > 17);
                let (mut control, mut callback) = active(&source, 3_997, true);
                let fence = control.capture_fresh_runtime_fence().unwrap();
                let revision = control
                    .publish_with_policy(
                        candidate(&source),
                        PublicationPolicy::InstallQuantizedSeekWhenReady {
                            fence,
                            quantization: LaunchQuantization::Quarter,
                        },
                    )
                    .unwrap();
                assert_eq!(
                    control.cancel_fresh_runtime_publication(revision),
                    FreshRuntimeCancellationResult::Cancelled
                );
                render_checked(&mut callback, &mut [[0.0; 2]; 17]);
                assert_eq!(control.status.position.load(Relaxed), 4_014);
                assert_eq!(control.status.audible_position.load(Relaxed), 3_997);
                assert_eq!(
                    control.status.rejected_revision(),
                    Some((revision, PlaybackHandoffRejection::FreshRuntimeCancelled))
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn section_cut_clears_history_when_performance_clock_matches() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let source = source_with_latency(true);
                assert!(source.arrangement().main_latency_frames() > 17);
                for (prerender, expected) in [(97, 4_014), (100, 4_017)] {
                    let (mut control, mut callback) = active(&source, 3_900, true);
                    render_checked(&mut callback, &mut vec![[0.0; 2]; prerender]);
                    let fence = control.capture_fresh_runtime_fence().unwrap();
                    control
                        .publish_with_policy(
                            candidate_in_region(
                                &source,
                                LoopRegion::new(4_000, 4_064, true).unwrap(),
                            ),
                            PublicationPolicy::InstallQuantizedSeekWhenReady {
                                fence,
                                quantization: LaunchQuantization::Quarter,
                            },
                        )
                        .unwrap();
                    render_checked(&mut callback, &mut [[0.0; 2]; 17]);
                    assert_eq!(control.status.position.load(Relaxed), expected);
                    assert_eq!(control.status.audible_position.load(Relaxed), 4_000);
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn ready_section_seek_splits_the_actual_callback_at_the_next_boundary() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let source = source();
                for (at, prefix) in [(3_997, 3_usize), (4_000, 0), (4_003, 3_997)] {
                    for block in [1, 17, 127, 257] {
                        let (mut control, mut callback) = active(&source, at, true);
                        let fence = control.capture_fresh_runtime_fence().unwrap();
                        let revision = control
                            .publish_with_policy(
                                candidate(&source),
                                PublicationPolicy::InstallQuantizedSeekWhenReady {
                                    fence,
                                    quantization: LaunchQuantization::Quarter,
                                },
                            )
                            .unwrap();
                        let mut output = vec![[0.0; 2]; prefix + 33];
                        for chunk in output.chunks_mut(block) {
                            render_checked(&mut callback, chunk);
                        }
                        let old_level = if at < 4_000 { 0.125 } else { 0.25 };
                        assert_eq!(
                            &output[..prefix],
                            vec![[old_level; 2]; prefix],
                            "old prefix at={at} block={block}"
                        );
                        assert_eq!(
                            &output[prefix..],
                            &[[0.625; 2]; 33],
                            "new suffix at={at} block={block}"
                        );
                        assert_eq!(control.status.applied_revision.load(Acquire), revision);
                        assert_eq!(control.status.position.load(Relaxed), 16_033);
                        assert_eq!(
                            control.status.performance_frame.load(Relaxed),
                            16_033,
                            "ordinary seek reset clock"
                        );
                        assert_eq!(control.reclaim(), 1, "old graph retires on control");
                    }
                }
                let (mut control, mut callback) = active(&source, 3_997, false);
                let fence = control.capture_fresh_runtime_fence().unwrap();
                control
                    .publish_with_policy(
                        candidate(&source),
                        PublicationPolicy::InstallQuantizedSeekWhenReady {
                            fence,
                            quantization: LaunchQuantization::EightBars,
                        },
                    )
                    .unwrap();
                let mut output = [[0.0; 2]; 33];
                render_checked(&mut callback, &mut output);
                assert_eq!(
                    output, [[0.625; 2]; 33],
                    "stopped clock starts immediately when ready"
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn rounded_section_midi_overlay_install_and_cancel_have_no_callback_allocations() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use hex_project::{BeatTime, Tempo};
                let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
                editor
                    .execute(Command::SetTempoPoint {
                        beat: BeatTime::ZERO,
                        tempo: Tempo::new(123_000_000).unwrap(),
                    })
                    .unwrap();
                let Created::Track(track) = editor
                    .execute(Command::InsertTrackWithDevice {
                        name: "keys".into(),
                        before_track: None,
                        slot: hex_project::TrackDeviceSlot::Instrument,
                        state: hex_project::default_instrument_state().unwrap(),
                    })
                    .unwrap()
                else {
                    panic!("track")
                };
                let Created::Scene(scene) = editor
                    .execute(Command::AddScene {
                        name: "scene".into(),
                    })
                    .unwrap()
                else {
                    panic!("scene")
                };
                let Created::SessionClip(session_clip) = editor
                    .execute(Command::CreateSessionMidiClip {
                        track,
                        scene,
                        name: "source".into(),
                        source_length: BeatTime::from_quarters(4).unwrap(),
                    })
                    .unwrap()
                else {
                    panic!("clip")
                };
                let Created::Clip(arrangement_clip) = editor
                    .execute(Command::PlaceMidiClip {
                        track,
                        start: BeatTime::ZERO,
                        length: BeatTime::from_quarters(4).unwrap(),
                    })
                    .unwrap()
                else {
                    panic!("clip")
                };
                for clip in [session_clip, arrangement_clip] {
                    editor
                        .execute(Command::AddMidiNote {
                            clip,
                            start: BeatTime::ZERO,
                            duration: BeatTime::ONE,
                            pitch: 60,
                            channel: 0,
                            velocity: 100,
                        })
                        .unwrap();
                }
                let source = Arc::new(
                    PreparedRevision::prepare(
                        editor.project(),
                        &BTreeMap::new(),
                        Arc::new(SineTable::new()),
                        None,
                    )
                    .unwrap(),
                );
                let region = LoopRegion::new(0, 23_415, true).unwrap();
                let make_candidate = || {
                    Playback::from_interactive_arrangement_preroll(
                        SessionPlayback::prepare_section_arrangement_preroll(
                            Arc::clone(&source),
                            None,
                            region,
                            0,
                            &AtomicBool::new(false),
                        )
                        .unwrap(),
                    )
                    .unwrap()
                };
                // At123bpm this frame loop requires123000001 micro-bpm admission,
                // so the candidate owns newly allocated source/span/index metadata.
                let mut expected = [[0.0; 2]; 17];
                let mut old =
                    Playback::try_from_prepared_with_session(Arc::clone(&source)).unwrap();
                old.apply(PlaybackCommand::PlayFrom(23_413));
                old.render(&mut expected[..2]).unwrap();
                make_candidate().render(&mut expected[2..]).unwrap();
                assert!(expected[2..].iter().flatten().any(|sample| *sample != 0.0));
                let (mut control, mut callback) = active(&source, 23_413, true);
                let fence = control.capture_fresh_runtime_fence().unwrap();
                let revision = control
                    .publish_with_policy(
                        make_candidate(),
                        PublicationPolicy::InstallQuantizedSeekWhenReady {
                            fence,
                            quantization: LaunchQuantization::Quarter,
                        },
                    )
                    .unwrap();
                let mut output = [[0.0; 2]; 17];
                render_checked(&mut callback, &mut output);
                assert_eq!(output, expected);
                assert_eq!(control.status.applied_revision.load(Acquire), revision);
                assert_eq!(control.status.performance_frame.load(Relaxed), 15);
                assert_eq!(control.reclaim(), 1);
                control.request_shutdown();
                render_checked(&mut callback, &mut output);
                assert_eq!(
                    control.reclaim(),
                    1,
                    "installed overlay is destroyed on control after callback return"
                );

                let (mut control, mut callback) = active(&source, 1, true);
                let fence = control.capture_fresh_runtime_fence().unwrap();
                let revision = control
                    .publish_with_policy(
                        make_candidate(),
                        PublicationPolicy::InstallQuantizedSeekWhenReady {
                            fence,
                            quantization: LaunchQuantization::Quarter,
                        },
                    )
                    .unwrap();
                render_checked(&mut callback, &mut output);
                assert_eq!(
                    control.cancel_fresh_runtime_publication(revision),
                    FreshRuntimeCancellationResult::Cancelled
                );
                render_checked(&mut callback, &mut output);
                assert_eq!(control.status.applied_revision.load(Acquire), 1);
                assert_eq!(
                    control.reclaim(),
                    1,
                    "cancelled overlay is destroyed on control"
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn queued_section_seek_cancellation_and_priority_stop_keep_old_graph_custody() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let source = source();
                for stop in [false, true] {
                    let (mut control, mut callback) = active(&source, 1, true);
                    let fence = control.capture_fresh_runtime_fence().unwrap();
                    let revision = control
                        .publish_with_policy(
                            candidate(&source),
                            PublicationPolicy::InstallQuantizedSeekWhenReady {
                                fence,
                                quantization: LaunchQuantization::Quarter,
                            },
                        )
                        .unwrap();
                    let mut prefix = [[0.0; 2]; 32];
                    render_checked(&mut callback, &mut prefix);
                    assert_eq!(control.status.applied_revision.load(Acquire), 1);
                    assert_eq!(prefix, [[0.125; 2]; 32]);
                    if stop {
                        control.stop();
                    } else {
                        assert_eq!(
                            control.cancel_fresh_runtime_publication(revision),
                            FreshRuntimeCancellationResult::Cancelled
                        );
                    }
                    let mut after = [[0.0; 2]; 32];
                    render_checked(&mut callback, &mut after);
                    assert_eq!(control.status.applied_revision.load(Acquire), 1);
                    assert_eq!(control.status.rejected_revision().unwrap().0, revision);
                    assert_eq!(after, [[if stop { 0.0 } else { 0.125 }; 2]; 32]);
                    assert_eq!(
                        control.reclaim(),
                        1,
                        "cancelled candidate returns on control"
                    );
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
