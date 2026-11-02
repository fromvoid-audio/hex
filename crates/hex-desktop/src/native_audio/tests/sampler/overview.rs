//! Peak-only allocator/lifetime probes and actual fresh-load overview controls.
use super::*;

#[test]
fn source_overview_quotes_match_actual_heap_and_backing_is_freed_before_budget_returns() {
    use hex_session::source_overview_test_support::Fixture;
    fn before_release(usage: hex_session::SourceOverviewUsage, bins: usize, metadata: usize) {
        assert_eq!(usage.bin_capacity_bytes, 16);
        assert!(usage.metadata_bytes >= metadata);
        // The outer counter starts at zero. If this same-size probe allocation
        // precedes freeing the old Box/Vec, its positive high-water detects it.
        if bins != 0 {
            assert_eq!(bins, 16);
            drop(std::hint::black_box(Box::new([0_u8; 16])));
        } else {
            const ROW: usize = std::mem::size_of::<(
                hex_project::AssetId,
                Arc<hex_session::OriginalSourceOverview>,
            )>();
            drop(std::hint::black_box(Box::new([0_u8; ROW])));
        }
    }
    let mut editor = hex_project::Editor::new(hex_project::Project::new(8000).unwrap(), 1).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "original.wav".into(),
            sample_rate: 8000,
            frames: 1,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let mut fixture = None;
    let measured = allocation_counter::measure(|| {
        fixture = Some(
            Fixture::new(
                asset,
                &[[0.5, -0.25]],
                8000,
                hex_project::OriginalSourceFacts::new(1, [7; 32]).unwrap(),
                Some(before_release),
            )
            .unwrap(),
        );
    });
    let fixture = fixture.as_mut().unwrap();
    let usage = fixture.usage();
    let actual = (usage.bin_capacity_bytes + usage.metadata_bytes) as u64;
    assert_eq!(measured.bytes_total, actual);
    assert_eq!(measured.bytes_current, actual as i64);
    let table_drop = allocation_counter::measure(|| fixture.drop_table());
    assert_eq!(table_drop.count_total, 1);
    assert_eq!(
        table_drop.bytes_total,
        std::mem::size_of::<(
            hex_project::AssetId,
            Arc<hex_session::OriginalSourceOverview>
        )>() as u64
    );
    assert_eq!(
        table_drop.bytes_max, 0,
        "table Vec must be freed before its release observer allocates"
    );
    assert_eq!(table_drop.count_current, -2);
    assert_eq!(fixture.usage().bin_capacity_bytes, 16);
    let peak_drop = allocation_counter::measure(|| fixture.drop_overview());
    assert_eq!(peak_drop.count_total, 1);
    assert_eq!(peak_drop.bytes_total, 16);
    assert_eq!(
        peak_drop.bytes_max, 0,
        "peak Box must be freed before its release observer allocates"
    );
    assert_eq!(peak_drop.count_current, -2);
    assert_eq!(fixture.usage().bin_capacity_bytes, 0);
}

#[test]
fn detailed_source_overview_heap_quote_and_last_release_match_actual_capacity() {
    use hex_session::source_overview_test_support::Fixture;
    let mut editor = hex_project::Editor::new(hex_project::Project::new(8000).unwrap(), 1).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "original.wav".into(),
            sample_rate: 8000,
            frames: 8195,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    let frames = vec![[0.5, -0.25]; 8195];
    let mut fixture = None;
    let measured = allocation_counter::measure(|| {
        fixture = Some(
            Fixture::new(
                asset,
                &frames,
                8000,
                hex_project::OriginalSourceFacts::new(1, [7; 32]).unwrap(),
                None,
            )
            .unwrap(),
        );
    });
    let fixture = fixture.as_mut().unwrap();
    let usage = fixture.usage();
    assert_eq!(usage.bin_capacity_bytes, 4096 * 16);
    assert_eq!(
        measured.bytes_total,
        (usage.bin_capacity_bytes + usage.metadata_bytes) as u64
    );
    assert_eq!(measured.bytes_current, measured.bytes_total as i64);
    fixture.drop_table();
    let before_release = fixture.usage();
    let released = allocation_counter::measure(|| fixture.drop_overview());
    assert_eq!(released.count_total, 0);
    assert_eq!(released.bytes_total, 0);
    assert_eq!(released.count_current, -2);
    assert_eq!(
        released.bytes_current,
        -((before_release.bin_capacity_bytes + before_release.metadata_bytes
            - fixture.usage().metadata_bytes) as i64)
    );
    assert_eq!(fixture.usage().bin_capacity_bytes, 0);
}

pub(super) fn fresh(
    cx: &gpui::TestAppContext,
    visual: &mut gpui::VisualTestContext,
    h: &Harness,
    window: gpui::AnyWindowHandle,
    end: Instant,
    track: hex_project::TrackId,
) -> hex_ui::SamplerSourceTestView {
    use gpui::{Modifiers, MouseButton};
    use hex_engine::SamplerParameter::{EndFrame, LoopEndFrame, LoopStartFrame, StartFrame};
    let track_selector = Box::leak(format!("track-name-{}", track.value()).into_boxed_str());
    let center = visual.debug_bounds(track_selector).unwrap().center();
    visual.simulate_click(center, Modifiers::default());
    visual.run_until_parked();
    let initial = cx
        .update(|cx| hex_ui::sampler_source_test_view(window, track, cx))
        .unwrap()
        .unwrap();
    assert!(
        initial.original_source.is_none(),
        "fresh canonical import has not been saved/opened"
    );
    assert_eq!((initial.source_frames, initial.playback_frames), (4, 8));
    let overview = initial.overviews.get(initial.asset).unwrap();
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
            .map(|f| f.map(|s| hex_session::CachedPeak { min: s, max: s }))
            .to_vec()
    );
    bounds(visual, initial.device, 0.0, 4.0, 0.0, 4.0);
    let mut start = 0.0;
    let mut end_frame = 4.0;
    let mut loop_start = 0.0;
    let mut loop_end = 4.0;
    for (index, (parameter, value)) in [
        (StartFrame, 1.0),
        (StartFrame, 0.0),
        (EndFrame, 3.0),
        (EndFrame, 4.0),
        (LoopStartFrame, 1.0),
        (LoopStartFrame, 0.0),
        (LoopEndFrame, 3.0),
        (LoopEndFrame, 4.0),
    ]
    .into_iter()
    .enumerate()
    {
        let selector = Box::leak(
            format!(
                "sampler-{}-{}-value",
                parameter.key(),
                initial.device.value()
            )
            .into_boxed_str(),
        );
        let center = visual
            .debug_bounds(selector)
            .expect("fresh source-frame input must be usable before Save/Open")
            .center();
        let old = h
            .backend
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .rendered_current(h)
            .unwrap();
        visual.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(&value.to_string());
        visual.simulate_keystrokes("enter");
        loop {
            pulse(cx, visual, h);
            if h.backend
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|b| b.status())
                .and_then(|s| s.rendered_current(h))
                .is_some_and(|s| {
                    s.context.editor_revision == old.context.editor_revision + 1
                        && s.context.project_generation == old.context.project_generation
                        && s.output_identity == old.output_identity
                        && s.audio_revision > old.audio_revision
                })
            {
                break;
            }
            assert!(
                Instant::now() < end,
                "fresh {parameter:?} {value}: {:?}",
                cx.update(|cx| hex_ui::sampler_keyboard_test_status(window, cx))
            );
        }
        for _ in 0..8 {
            pulse(cx, visual, h);
        }
        match parameter {
            StartFrame => start = value,
            EndFrame => end_frame = value,
            LoopStartFrame => loop_start = value,
            LoopEndFrame => loop_end = value,
            _ => panic!("fixture parameter"),
        }
        let current = cx
            .update(|cx| hex_ui::sampler_source_test_view(window, track, cx))
            .unwrap()
            .unwrap();
        assert!(current.original_source.is_none());
        assert_eq!(current.revision, initial.revision + index as u64 + 1);
        assert!(
            current.overviews.shares_storage_with(&initial.overviews),
            "ordinary control edits must reuse the cached table"
        );
        assert!(
            current
                .overviews
                .matches_context(hex_engine::AudioSourceContext {
                    project_generation: current.generation,
                    editor_revision: current.revision
                })
        );
        bounds(
            visual,
            initial.device,
            start,
            end_frame,
            loop_start,
            loop_end,
        );
        let play = visual
            .debug_bounds("play or pause · space")
            .unwrap()
            .center();
        visual.simulate_click(play, Modifiers::default());
        visual.run_until_parked();
        let mut output = [[0.0; 2]; 64];
        render(h, &mut output);
        let expected: &[[f32; 2]] = if start == 1.0 {
            &[[0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]]
        } else {
            &[[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]]
        };
        for (i, frame) in output.iter().enumerate() {
            assert_eq!(
                *frame,
                expected.get(i).copied().unwrap_or([0.0; 2]),
                "fresh {parameter:?} frame{i}"
            );
        }
        let stop = visual.debug_bounds("stop · escape").unwrap().center();
        visual.simulate_click(stop, Modifiers::default());
        visual.run_until_parked();
        render(h, &mut output);
        assert!(
            !h.backend
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert_eq!(output, [[0.0; 2]; 64]);
        for _ in 0..4 {
            pulse(cx, visual, h);
        }
    }
    initial
}

pub(super) fn reopened(
    cx: &gpui::TestAppContext,
    visual: &mut gpui::VisualTestContext,
    window: gpui::AnyWindowHandle,
    track: hex_project::TrackId,
    fresh: &hex_ui::SamplerSourceTestView,
) {
    let selector = Box::leak(format!("track-name-{}", track.value()).into_boxed_str());
    let center = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(center, gpui::Modifiers::default());
    visual.run_until_parked();
    let current = cx
        .update(|cx| hex_ui::sampler_source_test_view(window, track, cx))
        .unwrap()
        .unwrap();
    assert_eq!(
        current.original_source,
        Some(fresh.overviews.get(fresh.asset).unwrap().content())
    );
    assert_eq!((current.source_frames, current.playback_frames), (4, 8));
    assert_eq!(current.generation, fresh.generation + 2);
    assert!(
        !current.overviews.shares_storage_with(&fresh.overviews),
        "reopen reconstructs verified peaks while retained old peak-only view stays alive"
    );
    assert_eq!(
        current
            .overviews
            .get(current.asset)
            .unwrap()
            .peaks()
            .copied()
            .collect::<Vec<_>>(),
        fresh
            .overviews
            .get(fresh.asset)
            .unwrap()
            .peaks()
            .copied()
            .collect::<Vec<_>>()
    );
    bounds(visual, current.device, 0.0, 4.0, 0.0, 4.0);
}
fn bounds(
    visual: &mut gpui::VisualTestContext,
    device: hex_project::DeviceId,
    start: f64,
    end: f64,
    loop_start: f64,
    loop_end: f64,
) {
    let selector = Box::leak(format!("instrument-device-{}", device.value()).into_boxed_str());
    let frame = visual.debug_bounds(selector).unwrap();
    let overview = visual.debug_bounds("sampler-source-overview").unwrap();
    let waveform = visual
        .debug_bounds("sampler-source-waveform")
        .expect("actual original-source waveform must be rendered");
    let grid = visual.debug_bounds("sampler-controls").unwrap();
    assert!(
        overview.left() >= frame.left()
            && overview.right() <= frame.right()
            && overview.bottom() <= grid.top()
            && overview.bottom() <= frame.bottom()
    );
    assert!(grid.right() <= frame.right() && grid.bottom() <= frame.bottom());
    assert!(f32::from(waveform.size.width) >= 900.0);
    assert!(f32::from(waveform.size.height) >= 96.0);
    assert!(grid.size.height <= gpui::px(30.0));
    for (selector, value) in [
        ("sampler-source-start", start),
        ("sampler-source-end", end),
        ("sampler-source-loop-start", loop_start),
        ("sampler-source-loop-end", loop_end),
    ] {
        let marker = visual.debug_bounds(selector).unwrap();
        let offset = f32::from(marker.left() - waveform.left());
        assert!(
            (offset - (f32::from(waveform.size.width) - 1.0) * value as f32 / 4.0).abs() <= 0.5,
            "{selector} uses original4-frame coordinates, not converted8: {offset}"
        );
        assert!(marker.left() >= waveform.left() && marker.right() <= waveform.right());
    }
    for parameter in [
        hex_engine::SamplerParameter::Gain,
        hex_engine::SamplerParameter::Transpose,
        hex_engine::SamplerParameter::StartFrame,
        hex_engine::SamplerParameter::EndFrame,
        hex_engine::SamplerParameter::LoopEnabled,
        hex_engine::SamplerParameter::LoopStartFrame,
        hex_engine::SamplerParameter::LoopEndFrame,
        hex_engine::SamplerParameter::LoopCrossfadeMs,
        hex_engine::SamplerParameter::AttackMs,
        hex_engine::SamplerParameter::DecayMs,
        hex_engine::SamplerParameter::SustainPercent,
        hex_engine::SamplerParameter::ReleaseMs,
    ] {
        let selector = Box::leak(
            format!("sampler-{}-{}-value", parameter.key(), device.value()).into_boxed_str(),
        );
        let field = visual
            .debug_bounds(selector)
            .expect("all original-source controls must exist before Save/Open");
        assert!(
            field.left() >= grid.left()
                && field.right() <= grid.right()
                && field.bottom() <= frame.bottom(),
            "{parameter:?} clipped"
        );
    }
}
fn pulse(cx: &gpui::TestAppContext, visual: &mut gpui::VisualTestContext, h: &Harness) {
    cx.background_executor
        .advance_clock(Duration::from_millis(16));
    visual.run_until_parked();
    let mut slot = h.backend.lock().unwrap();
    if let Some(backend) = slot.as_mut() {
        if backend.shutdown_requested() {
            slot.take();
        } else {
            let measured =
                allocation_counter::measure(|| backend.render(&mut [[0.0; 2]; 64]).unwrap());
            assert_zero(measured);
        }
    }
    drop(slot);
    std::thread::sleep(Duration::from_millis(1));
}
fn render(h: &Harness, output: &mut [[f32; 2]]) {
    let mut slot = h.backend.lock().unwrap();
    let backend = slot.as_mut().unwrap();
    let measured = allocation_counter::measure(|| backend.render(output).unwrap());
    assert_zero(measured);
}
fn assert_zero(measured: allocation_counter::AllocationInfo) {
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
