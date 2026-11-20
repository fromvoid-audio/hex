use super::*;
use crate::{
    BorrowedMediaPage, BorrowedScalarPage, PreparedRevision, ScalarFileIdentity,
    ScalarProjectionIdentity, ScalarSourceKey, SessionPlayback, SessionPlaybackCommand, SineTable,
};
use hex_devices::utility::{UTILITY_DEFINITION, parameter};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created};
use std::sync::Arc;

#[derive(Clone)]
struct Pages {
    key: ScalarSourceKey,
    values: [Vec<f32>; 2],
    coverage: [Vec<u64>; 2],
    missing: Option<u64>,
    requests: usize,
}
impl MediaPages for Pages {
    fn page(&mut self, _: crate::StreamSourceKey, _: u64) -> Option<BorrowedMediaPage<'_>> {
        None
    }
    fn request_page(&mut self, _: crate::StreamSourceKey, _: u64) {}
    fn scalar_page(
        &mut self,
        key: ScalarSourceKey,
        page_index: u64,
    ) -> Option<BorrowedScalarPage<'_>> {
        if key != self.key || self.missing == Some(page_index) {
            return None;
        }
        let index = usize::try_from(page_index).ok()?;
        Some(BorrowedScalarPage {
            key,
            page_index,
            values: self.values.get(index)?,
            coverage: self.coverage.get(index)?,
        })
    }
    fn request_scalar_page(&mut self, _: ScalarSourceKey, _: u64) {
        self.requests += 1;
    }
}
struct Catalog<const N: usize>([Pages; N]);
impl<const N: usize> MediaPages for Catalog<N> {
    fn page(&mut self, _: crate::StreamSourceKey, _: u64) -> Option<BorrowedMediaPage<'_>> {
        None
    }
    fn request_page(&mut self, _: crate::StreamSourceKey, _: u64) {}
    fn scalar_page(&mut self, key: ScalarSourceKey, index: u64) -> Option<BorrowedScalarPage<'_>> {
        self.0
            .iter_mut()
            .find(|page| page.key == key)?
            .scalar_page(key, index)
    }
    fn request_scalar_page(&mut self, key: ScalarSourceKey, index: u64) {
        if let Some(page) = self.0.iter_mut().find(|page| page.key == key) {
            page.request_scalar_page(key, index);
        }
    }
}

fn source(
    map: &hex_project::TempoMap,
    device: hex_project::DeviceId,
    target: DeviceAutomationTarget,
) -> ScalarSourceMetadata {
    source_with_id(map, device, target, 1)
}

fn source_with_id(
    map: &hex_project::TempoMap,
    device: hex_project::DeviceId,
    target: DeviceAutomationTarget,
    id: u64,
) -> ScalarSourceMetadata {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hex_project::visit_automation_projection_identity(map, &mut |bytes| hash.update(bytes));
    ScalarSourceMetadata::new(
        ScalarSourceKey::new(7, id, 1).unwrap(),
        crate::ScalarSourceTarget::Device { device, target },
        48000,
        4094,
        4098,
        ScalarProjectionIdentity {
            version: hex_project::AUTOMATION_PROJECTION_VERSION,
            fingerprint: hash.finalize().into(),
        },
        ScalarFileIdentity {
            format_version: 2,
            file_bytes: 100,
            terminal_chain: [1; 32],
        },
    )
    .unwrap()
}
fn fixture(
    target: DeviceAutomationTarget,
) -> (
    crate::signal_graph::tests::Fixture,
    hex_project::DeviceId,
    Arc<PreparedRevision>,
    Pages,
) {
    let mut f = crate::signal_graph::tests::fixture(&[&[]], 8192);
    for audio in f.media.values_mut() {
        *audio = crate::Audio::new(48000, vec![[0.25, -0.5]; 8192]).unwrap();
    }
    let Created::Device(device) = f
        .editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: UTILITY_DEFINITION.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    let source = source(f.editor.project().tempo_map(), device, target);
    let key = source.key();
    let revision = Arc::new(
        PreparedRevision::prepare_with_scalar_automation(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
            ScalarAutomationBinding {
                device,
                target,
                source,
            },
        )
        .unwrap(),
    );
    let mut pages = Pages {
        key,
        values: [vec![0.0; 4096], vec![0.0; 2]],
        coverage: [vec![0; 64], vec![0; 1]],
        missing: None,
        requests: 0,
    };
    pages.coverage[0][63] = (1 << 62) | (1 << 63);
    pages.values[0][4095] = 1.0;
    pages.coverage[1][0] = 1 << 1;
    (f, device, revision, pages)
}
fn seek(runtime: &mut SessionPlayback) {
    runtime.apply(SessionPlaybackCommand::Seek(4094)).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
}

#[test]
fn scalar_origin_runs_split_missing_history_and_loop_discontinuities() {
    let origins = [
        None,
        None,
        Some(8),
        Some(9),
        Some(2),
        Some(3),
        None,
        Some(5),
    ];
    let mut runs = Vec::new();
    visit_origin_runs(
        origins.len(),
        |i| Ok(origins[i]),
        |offset, frame, len| {
            runs.push((offset, frame, len));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(runs, [(2, 8, 2), (4, 2, 2), (7, 5, 1)]);
    assert!(visit_origin_runs(257, |_| Ok(None), |_, _, _| Ok(())).is_err());
}

#[test]
fn recorded_provider_uses_delayed_stage_origin_for_demand_and_audio() {
    let target = DeviceAutomationTarget::Parameter(parameter::GAIN);
    let (mut f, device, _, mut pages) = fixture(target);
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: Some(device),
            state: crate::default_native_device_state(crate::DeviceTypeId::from_static(
                "hex.limiter",
            ))
            .unwrap(),
        })
        .unwrap();
    let binding = ScalarAutomationBinding {
        device,
        target,
        source: source(f.editor.project().tempo_map(), device, target),
    };
    let revision = Arc::new(
        PreparedRevision::prepare_with_scalar_automation(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
            binding,
        )
        .unwrap(),
    );
    let latency = revision.arrangement().main_latency_frames();
    assert!(latency > 0);
    let mut runtime = SessionPlayback::new(revision).unwrap();
    seek(&mut runtime);
    let mut demand = Vec::new();
    runtime
        .visit_ordinary_stream_chunk(4, None, &mut |extent| {
            demand.push(extent);
            Ok(())
        })
        .unwrap();
    assert!(
        demand.is_empty(),
        "no content has reached the delayed parameter stage"
    );
    let mut warmup = vec![[0.0; 2]; latency as usize];
    runtime.render_with_pages(&mut warmup, &mut pages).unwrap();
    runtime
        .visit_ordinary_stream_chunk(4, None, &mut |extent| {
            demand.push(extent);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        demand
            .iter()
            .map(|extent| (extent.source, extent.page_index, extent.start, extent.end))
            .collect::<Vec<_>>(),
        vec![
            (pages.key.into(), 0, 4094, 4096),
            (pages.key.into(), 1, 0, 2),
        ]
    );
    let mut output = [[9.0; 2]; 4];
    runtime.render_with_pages(&mut output, &mut pages).unwrap();
    assert_eq!(output[0], [0.0; 2]);
    assert!(output[1][0] > 0.25);
    assert!((output[2][0] - 0.25).abs() < 1e-6);
    assert_eq!(output[3], [0.0; 2]);
}

#[test]
fn delayed_host_input_and_output_match_stage_demand_seek_and_handoff() {
    for target in [
        DeviceAutomationTarget::HostInputGain,
        DeviceAutomationTarget::HostOutputGain,
    ] {
        let (mut f, utility, _, mut pages) = fixture(target);
        let Created::Device(limiter) = f
            .editor
            .execute(Command::InsertDevice {
                track: f.tracks[0],
                before: Some(utility),
                state: crate::default_native_device_state(crate::DeviceTypeId::from_static(
                    "hex.limiter",
                ))
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("limiter")
        };
        let binding = ScalarAutomationBinding {
            device: limiter,
            target,
            source: source(f.editor.project().tempo_map(), limiter, target),
        };
        let revision = Arc::new(
            PreparedRevision::prepare_with_scalar_automation(
                f.editor.project(),
                &f.media,
                Arc::new(SineTable::new()),
                None,
                binding,
            )
            .unwrap(),
        );
        let latency = revision.arrangement().main_latency_frames();
        assert!(latency > 0);
        // Keep the limiter below threshold so host gain anchors are independent
        // of its lookahead envelope. The provider test exercises maximum values.
        pages.values[0][4095] = 0.0;
        let mut oracle_pages = pages.clone();
        let mut oracle = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        seek(&mut oracle);
        let mut oracle_warmup = vec![[0.0; 2]; latency as usize];
        oracle
            .render_with_pages(&mut oracle_warmup, &mut oracle_pages)
            .unwrap();
        let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        seek(&mut runtime);
        let mut initial = Vec::new();
        runtime
            .visit_ordinary_stream_chunk(4, None, &mut |extent| {
                initial.push(extent);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            initial.is_empty(),
            target == DeviceAutomationTarget::HostOutputGain
        );
        let mut warmup = vec![[0.0; 2]; latency as usize];
        runtime.render_with_pages(&mut warmup, &mut pages).unwrap();
        let mut expected = Vec::new();
        runtime
            .visit_ordinary_stream_chunk(4, None, &mut |extent| {
                expected.push(extent);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            expected.is_empty(),
            target == DeviceAutomationTarget::HostInputGain
        );

        let mut candidate = SessionPlayback::new(revision).unwrap();
        let mut preview = Vec::new();
        candidate
            .preview_stream_handoff_with_scope(
                &mut runtime,
                4,
                None,
                &mut |extent| {
                    preview.push(extent);
                    Ok(())
                },
                true,
            )
            .unwrap();
        assert_eq!(preview, expected);
        candidate.try_inherit_state_from(&mut runtime).unwrap();
        let mut after = Vec::new();
        candidate
            .visit_ordinary_stream_chunk(4, None, &mut |extent| {
                after.push(extent);
                Ok(())
            })
            .unwrap();
        assert_eq!(after, expected);

        if target == DeviceAutomationTarget::HostOutputGain {
            pages.missing = Some(1);
            let before = candidate.arrangement_frame();
            let mut refused = [[9.0; 2]; 4];
            assert!(
                candidate
                    .render_with_pages(&mut refused, &mut pages)
                    .is_err()
            );
            assert_eq!(candidate.arrangement_frame(), before);
            assert_eq!(refused, [[0.0; 2]; 4]);
            pages.missing = None;
        }
        let mut output = [[0.0; 2]; 4];
        candidate
            .render_with_pages(&mut output, &mut pages)
            .unwrap();
        let mut uninterrupted = [[0.0; 2]; 4];
        oracle
            .render_with_pages(&mut uninterrupted, &mut oracle_pages)
            .unwrap();
        assert_eq!(
            output, uninterrupted,
            "{target:?} handoff must preserve live audio"
        );
        assert!(
            (output[0][0] - 0.00025).abs() < 1e-6,
            "{target:?}: {output:?}"
        );
        assert!((output[2][0] - 0.25).abs() < 1e-6, "{target:?}: {output:?}");

        let action = candidate
            .prepare_stream_action_in_scope(
                crate::playback::PlaybackAction::Session(SessionPlaybackCommand::Seek(4094)),
                true,
            )
            .unwrap();
        let mut seek_demand = Vec::new();
        candidate
            .visit_prepared_stream_action(&action, 4, &mut |extent| {
                seek_demand.push(extent);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            seek_demand.is_empty(),
            target == DeviceAutomationTarget::HostOutputGain
        );
    }
}

#[test]
fn delayed_scalar_values_follow_loop_origins_across_irregular_blocks() {
    let target = DeviceAutomationTarget::Parameter(parameter::GAIN);
    let (mut f, device, _, mut pages) = fixture(target);
    f.editor
        .execute(Command::SetLoopRegion {
            start: 4094,
            end: 4098,
            enabled: true,
        })
        .unwrap();
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: Some(device),
            state: crate::default_native_device_state(crate::DeviceTypeId::from_static(
                "hex.limiter",
            ))
            .unwrap(),
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_scalar_automation(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
            ScalarAutomationBinding {
                device,
                target,
                source: source(f.editor.project().tempo_map(), device, target),
            },
        )
        .unwrap(),
    );
    let latency = revision.arrangement().main_latency_frames();
    assert!(latency > 0);
    let mut runtime = SessionPlayback::new(revision).unwrap();
    seek(&mut runtime);
    let mut warmup = vec![[0.0; 2]; latency as usize];
    runtime.render_with_pages(&mut warmup, &mut pages).unwrap();
    let before = (runtime.arrangement_frame(), runtime.performance_frame());
    let mut projected = Vec::new();
    assert_eq!(
        runtime.visit_current_stream_quantum(8, None, true, &mut |extent| {
            projected.push(extent);
            Ok(())
        }),
        Ok(8)
    );
    assert_eq!(
        (runtime.arrangement_frame(), runtime.performance_frame()),
        before
    );
    let mut committed = Vec::new();
    let mut advanced = 0;
    while advanced < 8 {
        let window = runtime
            .visit_ordinary_stream_chunk(8 - advanced, None, &mut |extent| {
                committed.push(extent);
                Ok(())
            })
            .unwrap();
        assert!(window.frames > 0);
        runtime
            .render_with_pages(&mut vec![[0.0; 2]; window.frames], &mut pages)
            .unwrap();
        advanced += window.frames;
    }
    assert_eq!(
        projected, committed,
        "delayed scalar loop demand differs from committed chunks"
    );
    let mut actual = Vec::new();
    for width in [1, 3, 2, 2] {
        let mut block = vec![[0.0; 2]; width];
        runtime.render_with_pages(&mut block, &mut pages).unwrap();
        actual.extend(block);
    }
    for cycle in actual.chunks_exact(4) {
        assert_eq!(cycle[0], [0.0; 2]);
        assert!(cycle[1][0] > 0.25);
        assert!((cycle[2][0] - 0.25).abs() < 1e-6);
        assert_eq!(cycle[3], [0.0; 2]);
    }
    assert_eq!(pages.requests, 0);
}

#[test]
fn delayed_scalar_prewarm_projects_future_loop_origin_and_matches_due_render() {
    let target = DeviceAutomationTarget::Parameter(parameter::GAIN);
    let (mut f, device, _, mut pages) = fixture(target);
    f.editor
        .execute(Command::SetLoopRegion {
            start: 4092,
            end: 4099,
            enabled: true,
        })
        .unwrap();
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: Some(device),
            state: crate::default_native_device_state(crate::DeviceTypeId::from_static(
                "hex.limiter",
            ))
            .unwrap(),
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_scalar_automation(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
            ScalarAutomationBinding {
                device,
                target,
                source: source(f.editor.project().tempo_map(), device, target),
            },
        )
        .unwrap(),
    );
    let latency = revision.arrangement().main_latency_frames();
    assert!(latency > 0);
    let mut runtime = SessionPlayback::new(revision).unwrap();
    runtime.apply(SessionPlaybackCommand::Seek(4092)).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    runtime
        .render_with_pages(&mut [[0.0; 2]; 8], &mut pages)
        .unwrap();
    let command = SessionPlaybackCommand::StopTrack {
        track: f.tracks[0],
        quantization: crate::LaunchQuantization::Sixteenth,
    };
    let action = runtime
        .prepare_stream_action_in_scope(crate::PlaybackAction::Session(command), true)
        .unwrap();
    let due = action.scheduled_due_frame().unwrap();
    let mut predicted = Vec::new();
    let projected = runtime
        .visit_prepared_stream_action(&action, 3, &mut |extent| {
            predicted.push(extent);
            Ok(())
        })
        .unwrap();
    assert!(!predicted.is_empty());
    assert_ne!(
        predicted[0].page_index * 4096 + predicted[0].start as u64,
        projected.arrangement_frame,
        "delayed stage and incoming Arrangement cursor must differ"
    );
    runtime.apply(command).unwrap();
    let mut prewarm = Vec::new();
    runtime
        .visit_pending_stream_chunk_in_scope(
            due,
            3,
            &mut |extent| {
                prewarm.push(extent);
                Ok(())
            },
            true,
        )
        .unwrap();
    assert_eq!(prewarm, predicted);
    let advance = usize::try_from(due - runtime.performance_frame()).unwrap();
    runtime
        .render_with_pages(&mut vec![[0.0; 2]; advance], &mut pages)
        .unwrap();
    let mut actual = Vec::new();
    runtime
        .visit_ordinary_stream_chunk(3, None, &mut |extent| {
            actual.push(extent);
            Ok(())
        })
        .unwrap();
    assert_eq!(actual, predicted);
    let mut output = [[0.0; 2]; 3];
    runtime.render_with_pages(&mut output, &mut pages).unwrap();
    assert_eq!(pages.requests, 0);
}

#[test]
fn recorded_binding_refuses_overridden_shared_lane_until_reenabled() {
    for target in [
        DeviceAutomationTarget::Parameter(parameter::GAIN),
        DeviceAutomationTarget::HostOutputGain,
    ] {
        let (mut f, device, _, _) = fixture(target);
        f.editor
            .execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target,
                points: vec![hex_project::AutomationPoint {
                    beat: hex_project::BeatTime::ZERO,
                    value: hex_project::AutomationValue::new(0.5).unwrap(),
                    outgoing: hex_project::AutomationInterpolation::Step,
                }],
            })
            .unwrap();
        f.editor
            .execute(Command::SetDeviceAutomationReadSelection {
                device,
                target,
                selection: hex_project::AutomationReadSelection::Base,
            })
            .unwrap();
        let binding = ScalarAutomationBinding {
            device,
            target,
            source: source(f.editor.project().tempo_map(), device, target),
        };
        let result = PreparedRevision::prepare_with_scalar_automation(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
            binding.clone(),
        );
        assert_eq!(
            result.unwrap_err().0,
            "recorded automation target is manually overridden"
        );
        let mixed = PreparedRevision::prepare_with_scalar_automation_and_prepared_sources(
            f.editor.project(),
            &std::collections::BTreeMap::new(),
            1,
            Arc::new(SineTable::new()),
            None,
            binding.clone(),
        );
        assert_eq!(
            mixed.unwrap_err().0,
            "recorded automation target is manually overridden"
        );
        f.editor.execute(Command::ReenableDeviceAutomation).unwrap();
        assert!(
            PreparedRevision::prepare_with_scalar_automation(
                f.editor.project(),
                &f.media,
                Arc::new(SineTable::new()),
                None,
                binding,
            )
            .is_ok()
        );
    }
}

#[test]
#[ignore = "optimized scalar render profile; run explicitly with --release --ignored --nocapture"]
fn optimized_scalar_render_cost() {
    let target = DeviceAutomationTarget::Parameter(parameter::GAIN);
    let (f, device, _, mut pages) = fixture(target);
    let targets = [
        target,
        DeviceAutomationTarget::Parameter(parameter::MUTE),
        DeviceAutomationTarget::HostInputGain,
        DeviceAutomationTarget::HostOutputGain,
    ];
    let bindings = targets
        .into_iter()
        .enumerate()
        .map(|(index, target)| {
            let original = source_with_id(
                f.editor.project().tempo_map(),
                device,
                target,
                index as u64 + 1,
            );
            let source = ScalarSourceMetadata::new(
                original.key(),
                original.target(),
                original.project_rate(),
                0,
                8192,
                original.projection_identity(),
                original.file_identity(),
            )
            .unwrap();
            ScalarAutomationBinding {
                device,
                target,
                source,
            }
        })
        .collect();
    pages.values = [vec![0.25; 4096], vec![0.25; 4096]];
    pages.coverage = [vec![u64::MAX; 64], vec![u64::MAX; 64]];
    let mut pages = Catalog::<4>(std::array::from_fn(|index| {
        let mut source = pages.clone();
        source.key = ScalarSourceKey::new(7, index as u64 + 1, 1).unwrap();
        source
    }));
    let plain = PreparedRevision::prepare(
        f.editor.project(),
        &f.media,
        Arc::new(SineTable::new()),
        None,
    )
    .unwrap();
    let sources = f
        .media
        .iter()
        .map(|(&asset, audio)| {
            (
                asset,
                crate::compile::PreparedAudioSource::Resident(audio.clone()),
            )
        })
        .collect();
    let automated = PreparedRevision::prepare_with_scalar_automation_bindings(
        f.editor.project(),
        &sources,
        7,
        Arc::new(SineTable::new()),
        None,
        bindings,
        None,
    )
    .unwrap();
    for (label, revision) in [("inactive", plain), ("four active lanes", automated)] {
        let mut runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let mut output = [[0.0; 2]; 256];
        let mut elapsed = std::time::Duration::ZERO;
        for iteration in 0..51 {
            runtime.apply(SessionPlaybackCommand::Seek(0)).unwrap();
            let start = std::time::Instant::now();
            for _ in 0..32 {
                runtime.render_with_pages(&mut output, &mut pages).unwrap();
                std::hint::black_box(&output);
            }
            if iteration != 0 {
                elapsed += start.elapsed();
            }
        }
        eprintln!(
            "scalar render profile: {label}, 50 x 8192 frames, 256-frame blocks, {elapsed:?} total; runtime backing {} bytes; one warm-up iteration; seek and setup excluded; no native deadline claim",
            runtime.accounted_runtime_storage_bytes().unwrap(),
        );
    }
}

#[test]
#[ignore = "optimized delayed scalar render profile; run explicitly with --release --ignored --nocapture"]
fn optimized_delayed_scalar_render_cost() {
    let target = DeviceAutomationTarget::Parameter(parameter::GAIN);
    let (mut f, device, _, mut pages) = fixture(target);
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: Some(device),
            state: crate::default_native_device_state(crate::DeviceTypeId::from_static(
                "hex.limiter",
            ))
            .unwrap(),
        })
        .unwrap();
    let original = source(f.editor.project().tempo_map(), device, target);
    let source = ScalarSourceMetadata::new(
        original.key(),
        original.target(),
        original.project_rate(),
        0,
        8192,
        original.projection_identity(),
        original.file_identity(),
    )
    .unwrap();
    pages.values = [vec![0.5; 4096], vec![0.5; 4096]];
    pages.coverage = [vec![u64::MAX; 64], vec![u64::MAX; 64]];
    let plain = PreparedRevision::prepare(
        f.editor.project(),
        &f.media,
        Arc::new(SineTable::new()),
        None,
    )
    .unwrap();
    let automated = PreparedRevision::prepare_with_scalar_automation(
        f.editor.project(),
        &f.media,
        Arc::new(SineTable::new()),
        None,
        ScalarAutomationBinding {
            device,
            target,
            source,
        },
    )
    .unwrap();
    let latency = automated.arrangement().main_latency_frames();
    assert!(latency > 0);
    for (label, revision) in [("inactive", plain), ("active", automated)] {
        let mut runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let mut output = [[0.0; 2]; 256];
        let mut elapsed = std::time::Duration::ZERO;
        for iteration in 0..6 {
            runtime.apply(SessionPlaybackCommand::Seek(0)).unwrap();
            let start = std::time::Instant::now();
            for _ in 0..32 {
                runtime.render_with_pages(&mut output, &mut pages).unwrap();
                std::hint::black_box(&output);
            }
            if iteration != 0 {
                elapsed += start.elapsed();
            }
        }
        eprintln!(
            "delayed scalar render profile: {label}, latency {latency} frames, 5 x 8192 frames, 256-frame blocks, {elapsed:?} total; one warm-up iteration; seek and setup excluded; no native deadline claim"
        );
    }
}
#[test]
fn scalar_pages_drive_real_native_provider_and_live_clear_reveals_recorded_values() {
    let (_f, _device, revision, mut pages) =
        fixture(DeviceAutomationTarget::Parameter(parameter::GAIN));
    assert!(
        revision.contains_streamed_sources(),
        "scalar-only page dependency enters native preflight"
    );
    let mut legacy = crate::Renderer::new(revision.arrangement());
    legacy.play();
    let mut refused = [[9.0; 2]; 4];
    assert!(
        legacy.render(&mut refused).is_err(),
        "whole-project renderer must not omit recorded parameters"
    );
    assert_eq!(refused, [[0.0; 2]; 4]);
    drop(legacy);
    let mut runtime = SessionPlayback::new(revision).unwrap();
    seek(&mut runtime);
    let mut demand = Vec::new();
    let window = runtime
        .visit_next_stream_chunk(4, None, &mut |extent| {
            demand.push(extent);
            Ok(())
        })
        .unwrap();
    assert_eq!(window.frames, 4);
    assert_eq!(
        demand
            .iter()
            .map(|extent| (extent.source, extent.page_index, extent.start, extent.end))
            .collect::<Vec<_>>(),
        vec![
            (pages.key.into(), 0, 4094, 4096),
            (pages.key.into(), 1, 0, 2)
        ]
    );
    let mut output = [[9.0; 2]; 4];
    runtime.render_with_pages(&mut output, &mut pages).unwrap();
    assert_eq!(output[0], [0.0; 2], "covered zero is a real value");
    assert!(
        output[1][0] > 0.25,
        "normalized maximum reaches the actual provider"
    );
    assert_eq!(
        output[2],
        [0.25, -0.5],
        "uncovered frame restores immutable base"
    );
    assert_eq!(output[3], [0.0; 2]);
    seek(&mut runtime);
    runtime
        .set_native_parameter_input(hex_lattice::NativeTransientOperation::Set(
            hex_lattice::NativeTransientValue::Parameter {
                id: parameter::GAIN,
                value: ParameterValue::Float(2.0),
            },
        ))
        .unwrap();
    runtime.render_with_pages(&mut output, &mut pages).unwrap();
    assert_eq!(output, [[0.5, -1.0]; 4]);
    seek(&mut runtime);
    runtime
        .set_native_parameter_input(hex_lattice::NativeTransientOperation::Clear(
            DeviceAutomationTarget::Parameter(parameter::GAIN),
        ))
        .unwrap();
    runtime.render_with_pages(&mut output, &mut pages).unwrap();
    assert_eq!(output[0], [0.0; 2]);
    assert_eq!(output[2], [0.25, -0.5]);
}
#[test]
fn missing_cross_page_scalar_refuses_without_advancing_and_retries_same_window() {
    let (_, _, revision, mut pages) = fixture(DeviceAutomationTarget::Parameter(parameter::GAIN));
    let mut runtime = SessionPlayback::new(revision).unwrap();
    seek(&mut runtime);
    pages.missing = Some(1);
    let mut output = [[9.0; 2]; 4];
    assert!(runtime.render_with_pages(&mut output, &mut pages).is_err());
    assert_eq!(runtime.arrangement_frame(), 4094);
    assert_eq!(output, [[0.0; 2]; 4]);
    assert_eq!(pages.requests, 1);
    pages.missing = None;
    runtime.render_with_pages(&mut output, &mut pages).unwrap();
    assert_eq!(runtime.arrangement_frame(), 4098);
    assert_eq!(output[2], [0.25, -0.5]);
}
#[test]
fn scalar_host_stages_use_shared_host_mapping_and_stale_projection_refuses() {
    for target in [
        DeviceAutomationTarget::HostInputGain,
        DeviceAutomationTarget::HostOutputGain,
    ] {
        let (mut f, device, revision, mut pages) = fixture(target);
        let mut runtime = SessionPlayback::new(revision).unwrap();
        seek(&mut runtime);
        let mut output = [[9.0; 2]; 4];
        runtime.render_with_pages(&mut output, &mut pages).unwrap();
        assert_eq!(output[0], [0.00025, -0.0005]);
        assert_eq!(output[2], [0.25, -0.5]);
        let binding = ScalarAutomationBinding {
            device,
            target,
            source: source(f.editor.project().tempo_map(), device, target),
        };
        // Refuse stale projection before introducing any incompatible processor.
        f.editor
            .execute(Command::SetProjectTempo {
                tempo: hex_project::Tempo::new(150_000_000).unwrap(),
            })
            .unwrap();
        let error = PreparedRevision::prepare_with_scalar_automation(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
            binding,
        )
        .unwrap_err();
        assert_eq!(
            error.0,
            "scalar automation source rate or projection differs from project"
        );
        let current = ScalarAutomationBinding {
            device,
            target,
            source: source(f.editor.project().tempo_map(), device, target),
        };
        f.editor
            .execute(Command::InsertDevice {
                track: f.tracks[0],
                before: None,
                state: crate::default_native_device_state_for_class(
                    crate::NativeDeviceClass::Effect,
                    crate::DeviceTypeId::from_static("hex.limiter"),
                )
                .unwrap(),
            })
            .unwrap();
        let prepared = PreparedRevision::prepare_with_scalar_automation(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
            current,
        )
        .unwrap();
        assert!(prepared.arrangement().main_latency_frames() > 0);
    }
}

#[test]
fn staged_seek_and_candidate_handoff_plan_the_same_scalar_window_without_committing() {
    let (_, _, revision, mut pages) = fixture(DeviceAutomationTarget::Parameter(parameter::GAIN));
    let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let action = runtime
        .prepare_stream_action(crate::playback::PlaybackAction::Session(
            SessionPlaybackCommand::Seek(4094),
        ))
        .unwrap();
    let mut speculative = Vec::new();
    let window = runtime
        .visit_prepared_stream_action(&action, 4, &mut |extent| {
            speculative.push(extent);
            Ok(())
        })
        .unwrap();
    assert_eq!(window.arrangement_frame, 4094);
    assert_eq!(
        runtime.arrangement_frame(),
        0,
        "preflight cannot commit the seek"
    );
    runtime.validate_stream_action(&action).unwrap();
    runtime.apply(SessionPlaybackCommand::Seek(4094)).unwrap();
    let mut actual = Vec::new();
    runtime
        .visit_next_stream_chunk(4, None, &mut |extent| {
            actual.push(extent);
            Ok(())
        })
        .unwrap();
    assert_eq!(actual, speculative);
    let mut candidate = SessionPlayback::new(revision).unwrap();
    let mut handoff = Vec::new();
    let proposed = candidate
        .preview_stream_handoff(&mut runtime, 4, None, &mut |extent| {
            handoff.push(extent);
            Ok(())
        })
        .unwrap();
    assert_eq!(proposed.arrangement_frame, 4094);
    assert_eq!(handoff, actual);
    assert_eq!(runtime.arrangement_frame(), 4094);
    let mut output = [[0.0; 2]; 4];
    runtime.render_with_pages(&mut output, &mut pages).unwrap();
    assert_eq!(output[0], [0.0; 2]);
    assert_eq!(output[2], [0.25, -0.5]);
}

#[test]
fn grouped_scalar_pages_drive_same_and_distinct_processors_as_one_window() {
    let input = DeviceAutomationTarget::HostInputGain;
    let output = DeviceAutomationTarget::HostOutputGain;
    let (mut f, first, _, pages) = fixture(input);
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return bus");
    };
    let Created::Device(return_device) = f
        .editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: UTILITY_DEFINITION.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("return device");
    };
    let Created::Device(main_device) = f
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: UTILITY_DEFINITION.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("main device");
    };
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: bus,
            gain: 1.0,
        })
        .unwrap();
    let tempo = f.editor.project().tempo_map();
    let targets = [
        (first, input),
        (first, output),
        (return_device, input),
        (main_device, input),
    ];
    let bindings: Vec<_> = targets
        .into_iter()
        .enumerate()
        .rev()
        .map(|(index, (device, target))| ScalarAutomationBinding {
            device,
            target,
            source: source_with_id(tempo, device, target, index as u64 + 1),
        })
        .collect();
    let sources = f
        .media
        .iter()
        .map(|(&asset, audio)| {
            (
                asset,
                crate::compile::PreparedAudioSource::Resident(audio.clone()),
            )
        })
        .collect();
    let prepare = |bindings| {
        PreparedRevision::prepare_with_scalar_automation_bindings(
            f.editor.project(),
            &sources,
            7,
            Arc::new(SineTable::new()),
            None,
            bindings,
            Some((first, input)),
        )
    };
    assert_eq!(
        prepare(vec![bindings[0].clone(), bindings[0].clone()])
            .unwrap_err()
            .0,
        "duplicate scalar automation target"
    );
    let revision = Arc::new(prepare(bindings.clone()).unwrap());
    assert_eq!(revision.scalar_automation_bindings().count(), 4);

    let gain =
        [-6.0, -12.0, -18.0, -9.0].map(|db| hex_lattice_contract::HostGain::from_db(db).unwrap());
    let mut catalog = Catalog::<4>(std::array::from_fn(|index| {
        let mut page = pages.clone();
        page.key = ScalarSourceKey::new(7, index as u64 + 1, 1).unwrap();
        page.values[0][4094..].fill(gain[index].normalized());
        page.values[1].fill(gain[index].normalized());
        page
    }));
    let mut runtime = SessionPlayback::new(revision).unwrap();
    seek(&mut runtime);
    let mut demanded = std::collections::BTreeSet::new();
    runtime
        .visit_ordinary_stream_chunk(4, None, &mut |extent| {
            demanded.insert(extent.source);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        demanded,
        catalog.0.iter().map(|page| page.key.into()).collect()
    );

    catalog.0[2].missing = Some(1);
    let mut rendered = [[9.0; 2]; 4];
    assert!(
        runtime
            .render_with_pages(&mut rendered, &mut catalog)
            .is_err()
    );
    assert_eq!(runtime.arrangement_frame(), 4094);
    assert_eq!(rendered, [[0.0; 2]; 4]);
    catalog.0[2].missing = None;
    runtime
        .render_with_pages(&mut rendered, &mut catalog)
        .unwrap();
    let [track_input, track_output, return_gain, main_gain] =
        gain.map(hex_lattice_contract::HostGain::linear);
    let factor = track_input * track_output * (1.0 + return_gain) * main_gain;
    for frame in [0, 1, 3] {
        assert!(
            (rendered[frame][0] - 0.25 * factor).abs() < 1e-6,
            "{rendered:?}"
        );
        assert!(
            (rendered[frame][1] + 0.5 * factor).abs() < 1e-6,
            "{rendered:?}"
        );
    }
    assert_eq!(rendered[2], [0.5, -1.0]);

    seek(&mut runtime);
    let manual = hex_lattice_contract::HostGain::from_db(-3.0).unwrap();
    runtime
        .set_native_parameter_input(hex_lattice::NativeTransientOperation::Set(
            hex_lattice::NativeTransientValue::HostInputGain(manual),
        ))
        .unwrap();
    runtime
        .render_with_pages(&mut rendered, &mut catalog)
        .unwrap();
    let manual_factor = manual.linear() * track_output * (1.0 + return_gain) * main_gain;
    assert!(
        (rendered[0][0] - 0.25 * manual_factor).abs() < 1e-6,
        "{rendered:?}"
    );
}
