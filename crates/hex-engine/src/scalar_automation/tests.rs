use super::*;
use crate::{
    BorrowedMediaPage, BorrowedScalarPage, PreparedRevision, ScalarFileIdentity,
    ScalarProjectionIdentity, ScalarSourceKey, SessionPlayback, SessionPlaybackCommand, SineTable,
};
use hex_devices::utility::{UTILITY_DEFINITION, parameter};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created};
use std::sync::Arc;

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
fn source(map: &hex_project::TempoMap) -> ScalarSourceMetadata {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hex_project::visit_automation_projection_identity(map, &mut |bytes| hash.update(bytes));
    ScalarSourceMetadata::new(
        ScalarSourceKey::new(7, 1, 1).unwrap(),
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
    let source = source(f.editor.project().tempo_map());
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
        source: source(f.editor.project().tempo_map()),
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
            source: source(f.editor.project().tempo_map()),
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
    let (f, device, _, mut pages) = fixture(DeviceAutomationTarget::Parameter(parameter::GAIN));
    let original = source(f.editor.project().tempo_map());
    let source = ScalarSourceMetadata::new(
        original.key(),
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
            target: DeviceAutomationTarget::Parameter(parameter::GAIN),
            source,
        },
    )
    .unwrap();
    for (label, revision) in [("inactive", plain), ("active", automated)] {
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
            "scalar render profile: {label}, 50 x 8192 frames, 256-frame blocks, {elapsed:?} total; one warm-up iteration; seek and setup excluded; no native deadline claim"
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
            source: source(f.editor.project().tempo_map()),
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
            source: source(f.editor.project().tempo_map()),
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
