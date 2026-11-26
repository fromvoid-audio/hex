//! Waveform geometry cache and projection unit tests.
#[cfg(all(test, not(feature = "ui-test-support")))]
use super::cache::coarse_entry_key;
use super::cache::waveform_geometry_key;
use super::*;
use gpui::{point, size};
use std::cell::RefCell;

use crate::editor::waveform::waveform_column_count;
use std::time::Instant;

#[test]
fn frame_cache_keeps_held_overrange_through_commit_and_rechecks_replaced_source() {
    use hex_project::{ClipFade, Command, Created, Editor, Project};

    let mut model = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Imported { clip, .. } = model
        .execute(Command::ImportAudioTrack {
            name: "detail".into(),
            path: "detail.wav".into(),
            sample_rate: 48_000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("import detail clip");
    };
    let make_source = |sample| {
        let audio = hex_engine::Audio::new(48_000, vec![sample; 8]).unwrap();
        Arc::new(hex_media::waveform::summarize(&audio, 1).unwrap())
    };
    let old = make_source([2.0, -2.0]);
    let new = make_source([-0.5, 0.5]);
    let mapping = |model: &Editor, source: &CachedWaveform, fade| WaveformMapping::Frame {
        projection: Arc::new(
            waveform_frame::Projection::new(
                model.project(),
                model.project().clip(clip).unwrap(),
                source,
                fade,
                None,
                None,
            )
            .unwrap(),
        ),
        clock: waveform_frame::Clock::Arrangement,
        fallback: None,
    };
    let cache = RefCell::new(WaveformGeometryCache::default());
    let paint = |source: &Arc<CachedWaveform>, mapping: &WaveformMapping| {
        cached_waveform_geometry(
            &cache,
            source,
            &WaveformGeometryRequest {
                mapping,
                amplitude_scale: WaveformAmplitudeScale::default(),
                visible_start: 0,
                visible_end: 8,
                bounds: bounds(),
                scale: 1.0,
                columns: 8,
                grid: None,
            },
        )
        .unwrap()
        .0
    };
    let fade = ClipFade::new(8, 0, 5, 0).unwrap();
    let held = paint(&old, &mapping(&model, &old, Some(fade)));
    let WaveformOverrange::Ready(decisions) = &held.overrange else {
        panic!("detail decisions are available during the drag");
    };
    assert_eq!(decisions[0], Some(Overrange::default()));
    assert_eq!(
        decisions[7],
        Some(Overrange {
            positive: [true, false],
            negative: [false, true],
        })
    );
    model
        .execute(Command::SetClipFadeEnvelope { clip, fade })
        .unwrap();
    assert!(Arc::ptr_eq(
        &held,
        &paint(&old, &mapping(&model, &old, None))
    ));
    let replacement = paint(&new, &mapping(&model, &new, None));
    assert!(!Arc::ptr_eq(&held, &replacement));
    let WaveformOverrange::Ready(decisions) = &replacement.overrange else {
        panic!("replacement has its own exact decisions");
    };
    assert!(
        decisions
            .iter()
            .all(|decision| *decision == Some(Overrange::default()))
    );
}

fn waveform() -> Arc<CachedWaveform> {
    Arc::new(
        CachedWaveform::from_peaks(
            96_000,
            (0..4096)
                .map(|index| {
                    let phase = (index % 128) as f32 / 127.0;
                    [
                        CachedPeak {
                            min: -phase,
                            max: phase * 0.8,
                        },
                        CachedPeak {
                            min: -phase * 0.7,
                            max: phase,
                        },
                    ]
                })
                .collect(),
        )
        .expect("valid waveform fixture"),
    )
}

fn bounds() -> Bounds<Pixels> {
    Bounds::new(point(px(12.0), px(34.0)), size(px(1200.0), px(180.0)))
}

#[cfg(all(test, feature = "ui-unit-tier"))]
#[test]
fn full_queue_retry_wakes_once_without_spinning_offscreen() {
    use hex_project::{Command, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "clip".into(),
            path: "clip.wav".into(),
            sample_rate: 48_000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let clip = editor.project().clip(clip).unwrap().clone();
    let source = Arc::new(
        hex_media::waveform::summarize(
            &hex_engine::Audio::new(48_000, vec![[0.5; 2]; 8]).unwrap(),
            1,
        )
        .unwrap(),
    );
    let mapping = WaveformMapping::Clip {
        timeline_origin: 0,
        gain: None,
        fade: Some(clip.retained_fade()),
        clip: clip.clone(),
    };
    let request = WaveformGeometryRequest {
        mapping: &mapping,
        amplitude_scale: WaveformAmplitudeScale::default(),
        visible_start: 0,
        visible_end: 8,
        bounds: bounds(),
        scale: 1.0,
        columns: 2,
        grid: None,
    };
    let layout = WaveformLayout::new(&request).unwrap();
    let key = waveform_geometry_key(&mapping, 0, 8, bounds(), 1.0);
    let filler = super::super::waveform_overrange::Request {
        id: 0,
        slot: 0,
        waveform: Arc::clone(&source),
        mapping: waveform_overrange::Mapping::Clip {
            clip,
            timeline_origin: 0,
            gain: None,
            fade: None,
        },
        columns: std::iter::once(0..1).collect(),
    };
    let (worker, jobs, _replies) =
        super::super::waveform_overrange::Worker::saturated_for_test(&filler);
    let mut cache = OverrangeCache {
        worker: Some(worker),
        ..Default::default()
    };
    cache.schedule(&key, &source, &mapping, layout);
    assert!(cache.busy());
    assert!(cache.poll());
    assert!(!cache.busy());
    assert!(!cache.poll());
    jobs.try_recv().unwrap_or_else(|_| panic!("queued filler"));
    cache.schedule(&key, &source, &mapping, layout);
    assert_eq!(cache.pending, 1);
    cache.clear();
    assert!(!cache.busy());
}

#[cfg(all(test, feature = "ui-unit-tier"))]
#[test]
#[ignore = "optimized scheduling profile"]
fn overrange_scheduling_profile() {
    use hex_project::{Command, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "profile".into(),
            path: "profile.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let clip = editor.project().clip(clip).unwrap().clone();
    let source = Arc::new(
        hex_media::waveform::summarize(
            &hex_engine::Audio::new(48_000, vec![[0.75; 2]; 96_000]).unwrap(),
            1024,
        )
        .unwrap(),
    );
    let mapping = WaveformMapping::Clip {
        timeline_origin: 0,
        gain: None,
        fade: Some(clip.retained_fade()),
        clip,
    };
    let inputs: Vec<_> = (0..350)
        .map(|index| {
            let bounds = Bounds::new(
                point(px(index as f32 * 14.0), px(0.0)),
                size(px(9.3), px(44.0)),
            );
            let request = WaveformGeometryRequest {
                mapping: &mapping,
                amplitude_scale: WaveformAmplitudeScale::default(),
                visible_start: 0,
                visible_end: 96_000,
                bounds,
                scale: 2.0,
                columns: 19,
                grid: None,
            };
            (
                waveform_geometry_key(&mapping, 0, 96_000, bounds, 2.0),
                WaveformLayout::new(&request).unwrap(),
            )
        })
        .collect();
    let run = |enabled: bool| {
        let mut cache = OverrangeCache::default();
        if enabled {
            cache.requested = true;
            cache.poll();
        } else {
            cache.unavailable = true;
        }
        if enabled {
            let deadline = Instant::now() + std::time::Duration::from_secs(2);
            while cache.entries.len() < inputs.len() && Instant::now() < deadline {
                for (key, layout) in &inputs {
                    cache.schedule(key, &source, &mapping, *layout);
                }
                cache.poll();
                std::thread::yield_now();
            }
            assert_eq!(cache.entries.len(), inputs.len(), "profile working set");
        }
        let mut times = Vec::new();
        for _ in 0..20 {
            let start = Instant::now();
            for (key, layout) in &inputs {
                cache.schedule(key, &source, &mapping, *layout);
            }
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            cache.poll();
        }
        times.sort_by(f64::total_cmp);
        (times[10], times[19], cache.entries.len())
    };
    let baseline = run(false);
    let candidate = run(true);
    eprintln!(
        "350 waveform schedules, 20 frames: baseline p50/max {:.3}/{:.3} ms; candidate {:.3}/{:.3} ms, retained {}",
        baseline.0, baseline.1, candidate.0, candidate.1, candidate.2
    );
}

#[cfg(all(test, feature = "ui-unit-tier"))]
#[test]
#[ignore = "optimized retained geometry handoff profile"]
fn overrange_ready_rebuild_profile() {
    use hex_project::{Command, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "profile".into(),
            path: "profile.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let clip = editor.project().clip(clip).unwrap().clone();
    let source = Arc::new(
        hex_media::waveform::summarize(
            &hex_engine::Audio::new(48_000, vec![[0.75; 2]; 96_000]).unwrap(),
            1024,
        )
        .unwrap(),
    );
    let mapping = WaveformMapping::Clip {
        timeline_origin: 0,
        gain: None,
        fade: Some(clip.retained_fade()),
        clip,
    };
    let bounds: Vec<_> = (0..350)
        .map(|index| {
            Bounds::new(
                point(px(index as f32 * 14.0), px(0.0)),
                size(px(9.3), px(44.0)),
            )
        })
        .collect();
    let cache = RefCell::new(WaveformGeometryCache::default());
    for (index, bounds) in bounds.iter().copied().enumerate() {
        let key = waveform_geometry_key(&mapping, 0, 96_000, bounds, 2.0);
        cache
            .borrow_mut()
            .overrange
            .entries
            .push_back(OverrangeEntry {
                id: index as u64 + 1,
                key,
                source: Arc::clone(&source),
                columns: 19,
                state: OverrangeState::Pending,
            });
    }
    let render = || {
        for bounds in &bounds {
            let request = WaveformGeometryRequest {
                mapping: &mapping,
                amplitude_scale: WaveformAmplitudeScale::default(),
                visible_start: 0,
                visible_end: 96_000,
                bounds: *bounds,
                scale: 2.0,
                columns: 19,
                grid: None,
            };
            assert!(cached_waveform_geometry(&cache, &source, &request).is_some());
        }
    };
    let start = Instant::now();
    render();
    let initial = start.elapsed().as_secs_f64() * 1000.0;
    for entry in &mut cache.borrow_mut().overrange.entries {
        entry.state = OverrangeState::Ready(Arc::new(vec![Some(Overrange::default()); 19]));
    }
    let start = Instant::now();
    render();
    let rebuild = start.elapsed().as_secs_f64() * 1000.0;
    let mut warm = Vec::new();
    for _ in 0..20 {
        let start = Instant::now();
        render();
        warm.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    warm.sort_by(f64::total_cmp);
    eprintln!(
        "350 waveform geometries: initial pending {initial:.3} ms, ready rebuild {rebuild:.3} ms, warm p50/max {:.3}/{:.3} ms",
        warm[10], warm[19]
    );
}

#[cfg(all(test, feature = "ui-unit-tier"))]
#[test]
fn overrange_cache_keeps_preview_and_replaced_source_results_separate() {
    use hex_project::{Command, Created, Editor, Project};
    use std::time::Duration;

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "clip".into(),
            path: "clip.wav".into(),
            sample_rate: 48_000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("imported clip")
    };
    let clip = editor.project().clip(clip).unwrap().clone();
    let fade = hex_project::ClipFade::new(8, 0, 8, 0).unwrap();
    let canonical = WaveformMapping::Clip {
        clip: clip.clone(),
        timeline_origin: 0,
        gain: None,
        fade: Some(clip.retained_fade()),
    };
    let preview = WaveformMapping::Clip {
        clip,
        timeline_origin: 0,
        gain: None,
        fade: Some(fade),
    };
    let source = |sample| {
        Arc::new(
            hex_media::waveform::summarize(
                &hex_engine::Audio::new(48_000, vec![[sample; 2]; 8]).unwrap(),
                1,
            )
            .unwrap(),
        )
    };
    let old = source(2.0);
    let replacement = source(0.5);
    let cache = RefCell::new(WaveformGeometryCache::default());
    let request = |mapping| WaveformGeometryRequest {
        mapping,
        amplitude_scale: WaveformAmplitudeScale::default(),
        visible_start: 0,
        visible_end: 8,
        bounds: bounds(),
        scale: 1.0,
        columns: 2,
        grid: None,
    };
    let unrelated_mapping = WaveformMapping::Source { timeline_origin: 0 };
    let unrelated = cached_waveform_geometry(&cache, &old, &request(&unrelated_mapping))
        .unwrap()
        .0;
    let pending = cached_waveform_geometry(&cache, &old, &request(&canonical))
        .unwrap()
        .0;
    assert!(matches!(&pending.overrange, WaveformOverrange::Pending));
    assert!(cache.borrow().overrange_busy());
    assert!(cache.borrow_mut().poll_overrange()); // Starts the worker, outside paint.
    for (waveform, mapping) in [
        (&old, &canonical),
        (&old, &preview),
        (&replacement, &canonical),
    ] {
        cached_waveform_geometry(&cache, waveform, &request(mapping)).unwrap();
    }
    assert_eq!(cache.borrow().overrange.entries.len(), 3);
    let deadline = Instant::now() + Duration::from_millis(500);
    while cache.borrow().overrange_busy() {
        assert!(Instant::now() < deadline, "worker results did not arrive");
        cache.borrow_mut().poll_overrange();
        std::thread::yield_now();
    }
    let cache_ref = cache.borrow();
    let entries = &cache_ref.overrange.entries;
    assert_eq!(entries.len(), 3);
    let OverrangeState::Ready(canonical_columns) = &entries[0].state else {
        panic!("canonical result")
    };
    let OverrangeState::Ready(preview_columns) = &entries[1].state else {
        panic!("preview result")
    };
    assert_eq!(
        canonical_columns[0],
        Some(Overrange {
            positive: [true; 2],
            negative: [false; 2]
        })
    );
    assert_eq!(preview_columns[0], Some(Overrange::default()));
    for entry in entries {
        let OverrangeState::Ready(columns) = &entry.state else {
            panic!("ready result")
        };
        assert_eq!(columns.len(), 2);
        if Arc::ptr_eq(&entry.source, &replacement) {
            assert!(
                columns
                    .iter()
                    .all(|column| *column == Some(Overrange::default()))
            );
        }
    }
    drop(cache_ref);
    let installed = cached_waveform_geometry(&cache, &old, &request(&canonical))
        .unwrap()
        .0;
    assert!(!Arc::ptr_eq(&pending, &installed));
    assert_eq!(
        installed.columns, 4,
        "decision columns count against the geometry budget"
    );
    let WaveformOverrange::Ready(columns) = &installed.overrange else {
        panic!("installed canonical decision")
    };
    assert_eq!(
        columns[0],
        Some(Overrange {
            positive: [true; 2],
            negative: [false; 2]
        })
    );
    let installed_preview = cached_waveform_geometry(&cache, &old, &request(&preview))
        .unwrap()
        .0;
    let WaveformOverrange::Ready(columns) = &installed_preview.overrange else {
        panic!("installed preview decision")
    };
    assert_eq!(columns[0], Some(Overrange::default()));
    let installed_replacement =
        cached_waveform_geometry(&cache, &replacement, &request(&canonical))
            .unwrap()
            .0;
    let WaveformOverrange::Ready(columns) = &installed_replacement.overrange else {
        panic!("installed replacement decision")
    };
    assert_eq!(columns[0], Some(Overrange::default()));
    assert!(Arc::ptr_eq(
        &installed,
        &cached_waveform_geometry(&cache, &old, &request(&canonical))
            .unwrap()
            .0
    ));
    assert!(Arc::ptr_eq(
        &unrelated,
        &cached_waveform_geometry(&cache, &old, &request(&unrelated_mapping))
            .unwrap()
            .0
    ));
    cache.borrow_mut().clear();
    assert!(!cache.borrow().overrange_busy());
    assert!(cache.borrow().overrange.entries.is_empty());

    let unavailable_cache = RefCell::new(WaveformGeometryCache::default());
    unavailable_cache.borrow_mut().overrange.unavailable = true;
    let unavailable = cached_waveform_geometry(&unavailable_cache, &old, &request(&canonical))
        .unwrap()
        .0;
    assert!(matches!(
        &unavailable.overrange,
        WaveformOverrange::Unavailable
    ));

    let peak_only = Arc::new(CachedWaveform::from_peaks(8, old.bins().to_vec()).unwrap());
    let peak_cache = RefCell::new(WaveformGeometryCache::default());
    let pending = cached_waveform_geometry(&peak_cache, &peak_only, &request(&canonical))
        .unwrap()
        .0;
    assert!(matches!(&pending.overrange, WaveformOverrange::Pending));
    assert!(peak_cache.borrow_mut().poll_overrange());
    cached_waveform_geometry(&peak_cache, &peak_only, &request(&canonical)).unwrap();
    let deadline = Instant::now() + Duration::from_millis(500);
    while peak_cache.borrow().overrange_busy() {
        assert!(Instant::now() < deadline, "peak-only result did not arrive");
        peak_cache.borrow_mut().poll_overrange();
        std::thread::yield_now();
    }
    let result = cached_waveform_geometry(&peak_cache, &peak_only, &request(&canonical))
        .unwrap()
        .0;
    let WaveformOverrange::Ready(columns) = &result.overrange else {
        panic!("peak-only result")
    };
    assert_eq!(columns.as_slice(), &[None, None]);
}

#[cfg(all(test, feature = "ui-unit-tier"))]
#[test]
fn delayed_old_decision_rebuilds_only_its_own_geometry() {
    use hex_project::{Command, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "clip".into(),
            path: "clip.wav".into(),
            sample_rate: 48_000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let clip = editor.project().clip(clip).unwrap().clone();
    let old = WaveformMapping::Clip {
        timeline_origin: 0,
        gain: None,
        fade: Some(clip.retained_fade()),
        clip: clip.clone(),
    };
    let current = WaveformMapping::Clip {
        timeline_origin: 0,
        gain: None,
        fade: Some(hex_project::ClipFade::new(8, 0, 8, 0).unwrap()),
        clip,
    };
    let source = Arc::new(
        hex_media::waveform::summarize(
            &hex_engine::Audio::new(48_000, vec![[2.0; 2]; 8]).unwrap(),
            1,
        )
        .unwrap(),
    );
    let cache = RefCell::new(WaveformGeometryCache::default());
    let grid = WaveformGrid {
        start: 0,
        end: 8,
        left: 12.0,
        width: 1200.0,
        columns: 2,
        binned: true,
    };
    let request = |mapping| WaveformGeometryRequest {
        mapping,
        amplitude_scale: WaveformAmplitudeScale::default(),
        visible_start: 0,
        visible_end: 8,
        bounds: bounds(),
        scale: 1.0,
        columns: 2,
        grid: Some(grid),
    };
    for (id, mapping) in [(1, &old), (2, &current)] {
        let mut key = waveform_geometry_key(mapping, 0, 8, bounds(), 1.0);
        key.amplitude_scale = WaveformAmplitudeScale::default();
        if let WaveformMappingKey::Clip { id, .. } = &mut key.mapping {
            *id = None;
        }
        key.placement = WaveformLayout::new(&request(mapping))
            .unwrap()
            .coarse_placement(bounds())
            .unwrap()
            .key;
        cache
            .borrow_mut()
            .overrange
            .entries
            .push_back(OverrangeEntry {
                id,
                key,
                source: Arc::clone(&source),
                columns: 2,
                state: OverrangeState::Pending,
            });
    }
    let before_old = cached_waveform_geometry(&cache, &source, &request(&old))
        .unwrap()
        .0;
    let before_current = cached_waveform_geometry(&cache, &source, &request(&current))
        .unwrap()
        .0;
    {
        let mut cache = cache.borrow_mut();
        let old_entry = cache
            .overrange
            .entries
            .iter_mut()
            .find(|entry| entry.id == 1)
            .unwrap();
        old_entry.state = OverrangeState::Ready(Arc::new(vec![Some(Overrange::default()), None]));
    }
    let after_current = cached_waveform_geometry(&cache, &source, &request(&current))
        .unwrap()
        .0;
    assert!(Arc::ptr_eq(&before_current, &after_current));
    assert!(matches!(
        &after_current.overrange,
        WaveformOverrange::Pending
    ));
    let after_old = cached_waveform_geometry(&cache, &source, &request(&old))
        .unwrap()
        .0;
    assert!(!Arc::ptr_eq(&before_old, &after_old));
    let WaveformOverrange::Ready(columns) = &after_old.overrange else {
        panic!("old decision")
    };
    assert_eq!(columns.as_slice(), &[Some(Overrange::default()), None]);
    assert_eq!(
        cache.borrow().coarse_entry_counts.values().sum::<usize>(),
        2
    );
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn plain_clip_fade_shapes_peaks_and_changes_cache_identity() {
    use hex_project::{Command, Created, Editor, Project};

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "fade".into(),
            path: "fade.wav".into(),
            sample_rate: 48_000,
            frames: 100,
        })
        .unwrap()
    else {
        panic!("imported clip")
    };
    let neutral = editor.project().clip(clip).unwrap().clone();
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 40,
            fade_out: 40,
        })
        .unwrap();
    let faded = editor.project().clip(clip).unwrap().clone();
    let peak = CachedPeak {
        min: -1.0,
        max: 1.0,
    };
    let waveform = Arc::new(CachedWaveform::from_peaks(100, vec![[peak, peak]; 100]).unwrap());
    let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(100.0)));
    let mapping = |clip: &Clip| WaveformMapping::Clip {
        clip: clip.clone(),
        timeline_origin: 0,
        gain: None,
        fade: Some(clip.retained_fade()),
    };
    let held = WaveformMapping::Clip {
        clip: neutral.clone(),
        timeline_origin: 0,
        gain: None,
        fade: Some(faded.retained_fade()),
    };
    let cache = RefCell::new(WaveformGeometryCache::default());
    let get = |mapping: &WaveformMapping| {
        cached_waveform_geometry(
            &cache,
            &waveform,
            &WaveformGeometryRequest {
                amplitude_scale: crate::editor::waveform::WaveformAmplitudeScale::from_decibels(
                    false,
                ),
                mapping,
                visible_start: 0,
                visible_end: 100,
                bounds,
                scale: 1.0,
                columns: 100,
                grid: None,
            },
        )
        .unwrap()
        .0
    };
    let neutral = get(&mapping(&neutral));
    let faded = get(&mapping(&faded));
    assert!(
        !Arc::ptr_eq(&neutral, &faded),
        "fade edits invalidate cached geometry"
    );
    assert!(
        Arc::ptr_eq(&faded, &get(&held)),
        "held and canonical fades share the same geometry"
    );
    let neutral_top = &neutral.channels[0].outline[..100];
    let faded_top = &faded.channels[0].outline[..100];
    assert!(faded_top[0].y > neutral_top[0].y);
    assert_eq!(faded_top[50].y, neutral_top[50].y);
    assert!(faded_top[99].y > neutral_top[99].y);
    let neutral_bottom = &neutral.channels[0].outline[100..];
    let faded_bottom = &faded.channels[0].outline[100..];
    assert!(faded_bottom[0].y < neutral_bottom[0].y);
    assert_eq!(faded_bottom[50].y, neutral_bottom[50].y);
    assert!(faded_bottom[99].y < neutral_bottom[99].y);
}

fn key(origin: u64, bounds: Bounds<Pixels>) -> WaveformGeometryKey {
    waveform_geometry_key(
        &WaveformMapping::Source {
            timeline_origin: origin,
        },
        origin,
        origin + 96_000,
        bounds,
        2.0,
    )
}

fn coarse_key(clip_id: u64, first_frame: u64) -> WaveformGeometryKey {
    WaveformGeometryKey {
        reference_stereo: reference_stereo(),
        mapping: WaveformMappingKey::Clip {
            id: Some(clip_id),
            transformed: false,
            start: first_frame,
            duration: 256,
            source_offset: 0,
            source_length: 256,
            source_phase: 0,
            looping: false,
            timeline_origin: 0,
            gain: 0,
            fade: None,
        },
        amplitude_scale: WaveformAmplitudeScale::default(),
        height: 180.0_f32.to_bits(),
        scale: 1.0_f32.to_bits(),
        placement: WaveformPlacementKey::Coarse {
            first_frame,
            frames_per_bin: 128,
            bins: 2,
            pixel_pitch: 2.0_f32.to_bits(),
            detail_budget: 4_096,
        },
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn comparison_cache_identity_is_scoped_and_restored() {
    assert!(!reference_stereo());
    let current = key(0, bounds());
    {
        let _reference = use_reference_stereo(true);
        assert!(reference_stereo());
        assert_ne!(current, key(0, bounds()));
        {
            let _nested = use_reference_stereo(false);
            assert_eq!(current, key(0, bounds()));
        }
        assert!(reference_stereo());
    }
    assert!(!reference_stereo());
    assert_eq!(current, key(0, bounds()));
}

fn geometry(source: &CachedWaveform, origin: u64, bounds: Bounds<Pixels>) -> Arc<WaveformGeometry> {
    Arc::new(
        prepare_waveform_geometry(
            source,
            &WaveformMapping::Source {
                timeline_origin: origin,
            },
            origin,
            origin + 96_000,
            bounds,
            32,
            None,
        )
        .expect("waveform geometry"),
    )
}

#[test]
#[ignore = "manual release profile of repeated waveform geometry"]
fn profile_repeated_prepared_waveform() {
    use std::time::Instant;
    let mut samples = vec![[0.0, 0.0]; 16_000];
    samples[2_010] = [1.0, 1.0];
    let audio = hex_engine::Audio::new(48_000, samples).unwrap();
    let waveform = hex_media::waveform::summarize_repeated(
        &audio,
        hex_media::waveform::Repeat {
            offset: 0,
            period: 16_000,
            phase: 0,
        },
        1_600_000,
        1_024,
    )
    .unwrap();
    let mapping = WaveformMapping::Prepared {
        timeline_origin: 0,
        root_offset: 0,
        gain: hex_project::ClipGain::UNITY,
    };
    let extent = Bounds::new(point(px(0.0), px(0.0)), size(px(1000.0), px(100.0)));
    let mut times = Vec::with_capacity(100);
    for pass in 0..110 {
        let start = Instant::now();
        std::hint::black_box(
            prepare_waveform_geometry(&waveform, &mapping, 0, 80_000, extent, 1000, None).unwrap(),
        );
        let elapsed = start.elapsed().as_nanos();
        if pass >= 10 {
            times.push(elapsed);
        }
    }
    times.sort_unstable();
    eprintln!(
        "repeated waveform 1000 columns/5 cycles: p50={}ns p95={}ns",
        times[50], times[95]
    );
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn repeated_prepared_waveform_keeps_transients_in_their_visible_columns() {
    let mut samples = vec![[0.0, 0.0]; 16_000];
    samples[2_010] = [1.0, 1.0];
    let audio = hex_engine::Audio::new(48_000, samples).unwrap();
    let repeated = hex_media::waveform::summarize_repeated(
        &audio,
        hex_media::waveform::Repeat {
            offset: 0,
            period: 16_000,
            phase: 0,
        },
        1_600_000,
        1_024,
    )
    .unwrap();
    let extent = Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(100.0)));
    for (start, frames_per_column) in [
        (0_u64, 65_u64),
        (0, 160),
        (16_000 * 50, 160),
        (16_000 * 50, 1_000),
    ] {
        let geometry = prepare_waveform_geometry(
            &repeated,
            &WaveformMapping::Prepared {
                timeline_origin: 0,
                root_offset: 0,
                gain: hex_project::ClipGain::UNITY,
            },
            start,
            start + 100 * frames_per_column,
            extent,
            100,
            None,
        )
        .unwrap();
        let peaks: Vec<_> = geometry.channels[0].outline[..100]
            .iter()
            .enumerate()
            .filter(|(_, point)| point.y < px(25.0))
            .map(|(i, _)| i)
            .collect();
        let expected: Vec<_> = (0..100)
            .filter(|column| {
                let a = start + *column * frames_per_column;
                let b = a + frames_per_column;
                (a..b).any(|frame| frame % 16_000 == 2_010)
            })
            .map(|column| column as usize)
            .collect();
        assert_eq!(
            peaks, expected,
            "start {start}, frames per column {frames_per_column}"
        );
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn arrangement_clip_keeps_a_transient_in_its_visible_column() {
    use hex_project::{Command, Created, Editor, Project};

    let mut samples = vec![[0.0, 0.0]; 1_024_000];
    samples[2_010] = [1.0, 1.0];
    let audio = hex_engine::Audio::new(48_000, samples).unwrap();
    let waveform = hex_media::waveform::summarize(&audio, 1_024).unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "track".into(),
            path: "audio.wav".into(),
            sample_rate: 48_000,
            frames: 1_024_000,
        })
        .unwrap()
    else {
        panic!("imported audio")
    };
    let Created::Clip(id) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 6_500,
            source_offset: 0,
            source_length: 1_024_000,
            looping: false,
        })
        .unwrap()
    else {
        panic!("placed clip")
    };
    let extent = Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(100.0)));
    let geometry = prepare_waveform_geometry(
        &waveform,
        &WaveformMapping::Clip {
            clip: editor.project().clip(id).unwrap().clone(),
            timeline_origin: 0,
            gain: None,
            fade: None,
        },
        0,
        6_500,
        extent,
        100,
        None,
    )
    .unwrap();
    let peaks: Vec<_> = geometry.channels[0].outline[..100]
        .iter()
        .enumerate()
        .filter(|(_, point)| point.y < px(25.0))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(peaks, vec![2_010 / 65]);
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn event_detail_keeps_a_transient_in_its_visible_column() {
    // A long prepared root can have 1,000 source frames per cached bin.
    // At 65 frames per visible column, one impulse must not fill the
    // surrounding columns merely because they share a cached bin.
    let mut samples = vec![[0.0, 0.0]; 1_024_000];
    samples[2_010] = [1.0, 1.0];
    let audio = hex_engine::Audio::new(48_000, samples).unwrap();
    let waveform = hex_media::waveform::summarize(&audio, 1_024).unwrap();
    let extent = Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(100.0)));
    for (start, frames_per_column) in [
        (0_u64, 65_u64),
        (1_300, 65),
        (1_950, 65),
        (0, 1_000),
        (0, 1_025),
    ] {
        let geometry = prepare_waveform_geometry(
            &waveform,
            &WaveformMapping::GainedSource {
                timeline_origin: 0,
                gain: hex_project::ClipGain::UNITY.linear(),
                detail: true,
            },
            start,
            start + 100 * frames_per_column,
            extent,
            100,
            None,
        )
        .unwrap();
        let top = &geometry.channels[0].outline[..100];
        let peaks: Vec<_> = top
            .iter()
            .enumerate()
            .filter(|(_, p)| p.y < px(25.0))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(peaks, vec![((2_010 - start) / frames_per_column) as usize]);
    }

    // A longer root has wider cached bins. A narrow visible event may use
    // its share of the same total work budget rather than inheriting the
    // per-column limit of a full-width event.
    let mut long_samples = vec![[0.0, 0.0]; 3_072_000];
    long_samples[6_010] = [1.0, 1.0];
    let long_audio = hex_engine::Audio::new(48_000, long_samples).unwrap();
    let long_waveform = hex_media::waveform::summarize(&long_audio, 1_024).unwrap();
    let geometry = prepare_waveform_geometry(
        &long_waveform,
        &WaveformMapping::GainedSource {
            timeline_origin: 0,
            gain: hex_project::ClipGain::UNITY.linear(),
            detail: true,
        },
        0,
        205_000,
        extent,
        100,
        None,
    )
    .unwrap();
    let peaks: Vec<_> = geometry.channels[0].outline[..100]
        .iter()
        .enumerate()
        .filter(|(_, p)| p.y < px(25.0))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(peaks, vec![6_010 / 2_050]);
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn coarse_arranger_bins_keep_narrow_stereo_peaks_silence_and_dc() {
    use crate::editor::viewport::Viewport;
    let frames = 4_410_000_u64;
    let view = Viewport {
        start: 0,
        length: frames,
    };
    let grid = WaveformGrid::arrangement(view, 12.0, 200.0, 2.0, 44_100).unwrap();
    let step = (grid.end - grid.start) / grid.columns as u64;
    let extent = Bounds::new(point(px(12.0), px(34.0)), size(px(200.0), px(80.0)));
    for kind in ["needle", "silence", "dc"] {
        let mut peaks = vec![[CachedPeak { min: 0.0, max: 0.0 }; 2]; 4096];
        if kind == "needle" {
            peaks[2000][0] = CachedPeak {
                min: -0.7,
                max: 0.8,
            };
            peaks[15][1] = CachedPeak {
                min: -1.0,
                max: 0.35,
            };
        } else if kind == "dc" {
            peaks.fill([
                CachedPeak {
                    min: 0.25,
                    max: 0.25,
                },
                CachedPeak {
                    min: -0.375,
                    max: -0.375,
                },
            ]);
        }
        let source = CachedWaveform::from_peaks(frames as usize, peaks.clone()).unwrap();
        let geometry = prepare_waveform_geometry(
            &source,
            &WaveformMapping::Source { timeline_origin: 0 },
            0,
            frames,
            extent,
            400,
            Some(grid),
        )
        .unwrap();
        assert!(
            geometry.columns < 200,
            "overview still builds pixel-dense geometry"
        );
        for column in 0..geometry.columns / 2 {
            let start = grid.start + column as u64 * step;
            let end = start.saturating_add(step).min(frames);
            // Independent linear scan of the documented disjoint source
            // bins (the first remainder bins are one frame wider). No
            // production range aggregation establishes the answer.
            let mut expected: Option<[CachedPeak; 2]> = None;
            if start < end {
                let width = frames / peaks.len() as u64;
                let extra = frames % peaks.len() as u64;
                let mut bin_start = 0;
                for (index, peak) in peaks.iter().enumerate() {
                    let bin_end = bin_start + width + if (index as u64) < extra { 1 } else { 0 };
                    if start < bin_end && bin_start < end {
                        if let Some(old) = &mut expected {
                            for channel in 0..2 {
                                old[channel].min = old[channel].min.min(peak[channel].min);
                                old[channel].max = old[channel].max.max(peak[channel].max);
                            }
                        } else {
                            expected = Some(*peak);
                        }
                    }
                    bin_start = bin_end;
                }
            }
            let expected = expected.unwrap_or([CachedPeak { min: 0.0, max: 0.0 }; 2]);
            for (channel, peak) in expected.iter().enumerate() {
                let outline = &geometry.channels[channel].outline;
                let center = 34.0 + channel as f32 * 40.0 + 20.0;
                for edge in [column * 2, column * 2 + 1] {
                    assert!(
                        (f32::from(outline[edge].y) - (center - peak.max * 17.6)).abs() < 0.0001,
                        "{kind} upper peak lost"
                    );
                    assert!(
                        (f32::from(outline[edge + geometry.columns].y)
                            - (center - peak.min * 17.6))
                            .abs()
                            < 0.0001,
                        "{kind} lower peak lost"
                    );
                }
            }
        }
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn coarse_peak_remains_visible_inside_a_clip_narrower_than_a_bin() {
    use crate::editor::viewport::Viewport;
    let view = Viewport {
        start: 0,
        length: 4_410_000,
    };
    let grid = WaveformGrid::arrangement(view, 12.0, 1000.0, 2.0, 44_100).unwrap();
    let origin = 1_000_000;
    let duration = 8_000;
    let mut peaks = vec![[CachedPeak { min: 0.0, max: 0.0 }; 2]; duration];
    peaks[duration / 2] = [CachedPeak {
        min: -1.0,
        max: 1.0,
    }; 2];
    let source = CachedWaveform::from_peaks(duration, peaks).unwrap();
    let left = 12.0 + origin as f32 / view.length as f32 * 1000.0;
    let width = duration as f32 / view.length as f32 * 1000.0;
    let extent = Bounds::new(point(px(left), px(34.0)), size(px(width), px(80.0)));
    let geometry = prepare_waveform_geometry(
        &source,
        &WaveformMapping::Prepared {
            timeline_origin: origin,
            root_offset: 0,
            gain: hex_project::ClipGain::UNITY,
        },
        origin,
        origin + duration as u64,
        extent,
        waveform_column_count(width, 2.0),
        Some(grid),
    )
    .unwrap();
    let middle = left + width * 0.5;
    let outline = &geometry.channels[0].outline;
    let visible_peak = (0..geometry.columns.saturating_sub(1)).any(|i| {
        let a = outline[i];
        let b = outline[i + 1];
        f32::from(a.x) <= middle
            && middle <= f32::from(b.x)
            && f32::from(b.x) > f32::from(a.x)
            && (f32::from(a.y) - 36.4).abs() < 0.0001
            && (f32::from(b.y) - 36.4).abs() < 0.0001
            && (f32::from(outline[i + geometry.columns].y) - 71.6).abs() < 0.0001
            && (f32::from(outline[i + geometry.columns + 1].y) - 71.6).abs() < 0.0001
    });
    assert!(
        visible_peak,
        "clip masking hid a narrow transient between bin centers"
    );
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn coarse_waveforms_translate_without_resampling_during_pan() {
    use crate::editor::viewport::Viewport;
    let source = waveform();
    let length = 24_000;
    let first_start = 20_000;
    let extent = Bounds::new(point(px(12.0), px(34.0)), size(px(100.25), px(80.0)));
    let grid = WaveformGrid::arrangement(
        Viewport {
            start: first_start,
            length,
        },
        12.0,
        100.25,
        2.0,
        44_100,
    )
    .unwrap();
    let step = (grid.end - grid.start) / grid.columns as u64;
    let samples = |start| {
        let grid = WaveformGrid::arrangement(Viewport { start, length }, 12.0, 100.25, 2.0, 44_100)
            .unwrap();
        let geometry = prepare_waveform_geometry(
            &source,
            &WaveformMapping::Source { timeline_origin: 0 },
            start,
            start + length,
            extent,
            201,
            Some(grid),
        )
        .unwrap();
        let first = ((u128::from(start - grid.start) * grid.columns as u128)
            / u128::from(grid.end - grid.start)) as usize;
        let first = first.saturating_sub(1);
        std::array::from_fn::<_, 2, _>(|channel| {
            let outline = &geometry.channels[channel].outline;
            (0..geometry.columns / 2)
                .map(|i| {
                    (
                        grid.start + (first + i) as u64 * step,
                        (
                            f32::from(outline[i * 2].x),
                            f32::from(outline[i * 2].y).to_bits(),
                            f32::from(outline[i * 2 + geometry.columns].y).to_bits(),
                        ),
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>()
        })
    };
    let before = samples(first_start);
    for start in [first_start + step / 3, first_start + step * 2 + 7] {
        let after = samples(start);
        let displacement = (start - first_start) as f32 / length as f32 * 100.25;
        for channel in 0..2 {
            let mut compared = 0;
            for (bin, &(x, upper, lower)) in &before[channel] {
                if let Some(&(new_x, new_upper, new_lower)) = after[channel].get(bin) {
                    assert_eq!(
                        (upper, lower),
                        (new_upper, new_lower),
                        "pan resampled a fixed source bin"
                    );
                    assert!(
                        (new_x - x + displacement).abs() < 0.002,
                        "waveform did not translate with the viewport"
                    );
                    compared += 1;
                }
            }
            assert!(compared > 20);
        }
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn coarse_aligned_clips_share_only_equivalent_geometry() {
    use hex_project::{Command, Created, Editor, Project};

    let start = 96_000;
    let duration = 48_000;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: start,
        })
        .unwrap()
    else {
        panic!("imported source")
    };
    let place = |editor: &mut Editor, track, source_offset| {
        let Created::Clip(id) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start,
                duration,
                source_offset,
                source_length: duration,
                looping: false,
            })
            .unwrap()
        else {
            panic!("placed clip")
        };
        id
    };
    let first = place(&mut editor, track, 0);
    let others = [0, 1_024].map(|source_offset| {
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "copy".into(),
            })
            .unwrap()
        else {
            panic!("added track")
        };
        place(&mut editor, track, source_offset)
    });
    let source = waveform();
    let cache = RefCell::new(WaveformGeometryCache::default());
    let cached = |id, timeline_origin, y| {
        let mapping = WaveformMapping::Clip {
            clip: editor.project().clip(id).unwrap().clone(),
            timeline_origin,
            gain: None,
            fade: None,
        };
        cached_waveform_geometry(
            &cache,
            &source,
            &WaveformGeometryRequest {
                mapping: &mapping,
                amplitude_scale: WaveformAmplitudeScale::default(),
                visible_start: start,
                visible_end: start + duration,
                bounds: Bounds::new(point(px(202.25), px(y)), size(px(96.0), px(80.0))),
                scale: 2.0,
                columns: 144,
                grid: Some(WaveformGrid {
                    start: 0,
                    end: start + duration,
                    left: 10.25,
                    width: 288.0,
                    columns: 144,
                    binned: true,
                }),
            },
        )
        .unwrap()
    };
    let (first_geometry, first_offset) = cached(first, start, 34.25);
    let (second_geometry, second_offset) = cached(others[0], start, 134.25);
    let outline = &first_geometry.channels[0].outline;
    assert!(outline.iter().any(|point| point.y != outline[0].y));
    assert!(Arc::ptr_eq(&first_geometry, &second_geometry));
    assert_eq!(first_offset.x, second_offset.x);
    assert_eq!(second_offset.y - first_offset.y, px(100.0));
    assert_eq!(cache.borrow().entry_count(), 1);
    for (id, origin) in [(others[1], start), (others[0], start + 1)] {
        assert!(!Arc::ptr_eq(&first_geometry, &cached(id, origin, 134.25).0));
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn coarse_clip_cache_reuses_paths_across_aligned_grid_shifts() {
    use crate::editor::viewport::Viewport;
    use hex_project::{Command, Created, Editor, Project};

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "coarse".into(),
            path: "coarse.wav".into(),
            sample_rate: 48_000,
            frames: 24_000,
        })
        .unwrap()
    else {
        panic!("imported clip")
    };
    let mapping = WaveformMapping::Clip {
        clip: editor.project().clip(clip).unwrap().clone(),
        timeline_origin: 40_000,
        gain: None,
        fade: None,
    };
    let source = waveform();
    let cache = RefCell::new(WaveformGeometryCache::default());
    let grid = |start, length| {
        WaveformGrid::arrangement(Viewport { start, length }, 12.25, 400.25, 2.0, 48_000).unwrap()
    };
    let request = WaveformGeometryRequest {
        mapping: &mapping,
        amplitude_scale: WaveformAmplitudeScale::default(),
        visible_start: 40_000,
        visible_end: 64_000,
        bounds: Bounds::new(point(px(112.5), px(34.25)), size(px(120.0), px(80.0))),
        scale: 2.0,
        columns: 240,
        grid: Some(grid(20_000, 80_000)),
    };
    let (initial, initial_offset) = cached_waveform_geometry(&cache, &source, &request).unwrap();
    let panned = WaveformGeometryRequest {
        bounds: Bounds::new(point(px(109.5), px(18.75)), request.bounds.size),
        grid: Some(grid(21_203, 80_000)),
        ..request
    };
    assert_ne!(request.grid.unwrap().start, panned.grid.unwrap().start);
    let (retained, offset) = cached_waveform_geometry(&cache, &source, &panned).unwrap();
    assert!(Arc::ptr_eq(&initial, &retained));
    assert_ne!(initial_offset, offset);
    let fresh = prepare_waveform_geometry_with_scale(&source, &panned).unwrap();
    for (local, fresh) in retained.channels.iter().zip(&fresh.channels) {
        assert_eq!(local.outline.len(), fresh.outline.len());
        for (local, fresh) in local.outline.iter().zip(&fresh.outline) {
            assert!((f32::from(local.x + offset.x - fresh.x)).abs() < 0.001);
            assert_eq!(local.y + offset.y, fresh.y);
        }
    }

    for changed in [
        WaveformGeometryRequest {
            visible_start: 42_000,
            ..panned
        },
        WaveformGeometryRequest {
            grid: Some(grid(21_203, 72_000)),
            ..panned
        },
        WaveformGeometryRequest {
            bounds: Bounds::new(panned.bounds.origin, size(px(120.0), px(90.0))),
            ..panned
        },
        WaveformGeometryRequest {
            scale: 1.0,
            ..panned
        },
    ] {
        let (new_path, _) = cached_waveform_geometry(&cache, &source, &changed).unwrap();
        assert!(!Arc::ptr_eq(&initial, &new_path));
    }
    let (new_source, _) = cached_waveform_geometry(&cache, &waveform(), &panned).unwrap();
    assert!(!Arc::ptr_eq(&initial, &new_source));
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn coarse_cache_uses_exact_bin_identity_at_the_timeline_limit() {
    let request = WaveformGeometryRequest {
        mapping: &WaveformMapping::Source { timeline_origin: 0 },
        amplitude_scale: WaveformAmplitudeScale::default(),
        visible_start: u64::MAX - 700,
        visible_end: u64::MAX - 300,
        bounds: bounds(),
        scale: 2.0,
        columns: 10,
        grid: None,
    };
    let placement = |start, end| {
        WaveformLayout::new(&WaveformGeometryRequest {
            grid: Some(WaveformGrid {
                start,
                end,
                left: 8.0,
                width: 100.0,
                columns: 10,
                binned: true,
            }),
            ..request
        })
        .unwrap()
        .coarse_placement(request.bounds)
    };
    let first = placement(u64::MAX - 1_100, u64::MAX - 100).unwrap();
    let pinned = placement(u64::MAX - 1_000, u64::MAX).unwrap();
    assert_eq!(first.key, pinned.key);
    assert_ne!(first.offset, pinned.offset);
    assert!(placement(u64::MAX - 1_000, u64::MAX - 1).is_none());
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn resize_preserves_interior_stereo_outline_at_fractional_edges_and_column_cap() {
    let source = waveform();
    for (width, scale) in [(783.25, 1.0), (783.25, 2.0), (5000.25, 2.0)] {
        let grid = WaveformGrid {
            start: 0,
            end: 96_000,
            left: 125.25,
            width,
            columns: waveform_column_count(width, scale),
            binned: false,
        };
        for prepared in [false, true] {
            let draw = |start: u64, end: u64| {
                let mapping = if prepared {
                    // A retained prepared root's offset follows a left crop.
                    WaveformMapping::Prepared {
                        timeline_origin: start,
                        root_offset: start,
                        gain: hex_project::ClipGain::UNITY,
                    }
                } else {
                    WaveformMapping::Source { timeline_origin: 0 }
                };
                let extent = Bounds::new(
                    point(px(grid.left + start as f32 / 96_000.0 * width), px(34.0)),
                    size(px((end - start) as f32 / 96_000.0 * width), px(44.0)),
                );
                prepare_waveform_geometry(
                    &source,
                    &mapping,
                    start,
                    end,
                    extent,
                    waveform_column_count(f32::from(extent.size.width), scale),
                    Some(grid),
                )
                .unwrap()
            };
            let baseline = draw(4_900, 91_100);
            // Both trim directions, subpixel motion, crop and newly exposed
            // audio. Compare the actual outline points fed to PathBuilder.
            for (start, end) in [
                (5_001, 91_100),
                (4_900, 90_999),
                (6_103, 89_087),
                (4_003, 92_017),
            ] {
                let resized = draw(start, end);
                for channel in 0..2 {
                    let interior = |geometry: &WaveformGeometry| {
                        geometry.channels[channel]
                            .outline
                            .iter()
                            .filter(|p| {
                                let x = f32::from(p.x);
                                x > grid.left + width * 0.2 && x < grid.left + width * 0.8
                            })
                            .map(|p| (f32::from(p.x).to_bits(), f32::from(p.y).to_bits()))
                            .collect::<Vec<_>>()
                    };
                    let expected = interior(&baseline);
                    assert!(expected.len() > 100);
                    assert!(
                        interior(&resized) == expected,
                        "unchanged outline must not shift: width={width} scale={scale} prepared={prepared} crop={start}..{end}"
                    );
                }
                assert!(resized.columns <= 4096);
            }
            let revealed = draw(4_003, 92_017);
            assert!(
                revealed.channels[0]
                    .outline
                    .iter()
                    .any(|p| f32::from(p.x) < grid.left + width * 4_900.0 / 96_000.0)
            );
            assert!(
                revealed.channels[0]
                    .outline
                    .iter()
                    .any(|p| f32::from(p.x) > grid.left + width * 91_100.0 / 96_000.0)
            );
        }
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn prepared_crop_sampling_is_invariant_across_grid_bins_before_clip_origin() {
    let source = waveform();
    let grid = WaveformGrid {
        start: 0,
        end: 96_000,
        left: 125.25,
        width: 783.25,
        columns: 784,
        binned: false,
    };
    let extent = Bounds::new(
        point(px(125.25 + 5_001.0 / 96_000.0 * 783.25), px(34.0)),
        size(px(700.0), px(44.0)),
    );
    let draw = |origin, offset| {
        prepare_waveform_geometry(
            &source,
            &WaveformMapping::Prepared {
                timeline_origin: origin,
                root_offset: offset,
                gain: hex_project::ClipGain::UNITY,
            },
            5_001,
            91_100,
            extent,
            700,
            Some(grid),
        )
        .unwrap()
    };
    let full_root = draw(0, 0);
    let retained_crop = draw(5_001, 5_001);
    for channel in 0..2 {
        assert!(
            retained_crop.channels[channel].outline == full_root.channels[channel].outline,
            "same accepted root/time positions retain edge-bin peaks after a crop"
        );
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn canonical_trim_and_lower_fade_resize_keep_loop_and_one_shot_outline_still() {
    use hex_project::{Command, Created, Editor, Project, TrimEdge};
    let source = waveform();
    let mut fixture = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported {
        track,
        clip: imported,
        ..
    } = fixture
        .execute(Command::ImportAudioTrack {
            name: "audio source".into(),
            path: "missing.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("import result")
    };
    let project = fixture.project().clone();
    let asset = project.clip(imported).unwrap().asset();
    let mut editor = Editor::new(project, 8).unwrap();
    for looping in [false, true] {
        let Created::Clip(id) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 4_000,
                duration: 84_000,
                source_offset: 100,
                source_length: if looping { 1701 } else { 84_000 },
                looping,
            })
            .unwrap()
        else {
            panic!("placed clip")
        };
        let clip = editor.project().clip(id).unwrap();
        let grid = WaveformGrid {
            start: 0,
            end: 96_000,
            left: 125.25,
            width: 783.25,
            columns: 1567,
            binned: false,
        };
        let draw = |clip: &Clip| {
            let start = clip.start();
            let end = start + clip.duration();
            let extent = Bounds::new(
                point(
                    px(grid.left + start as f32 / 96_000.0 * grid.width),
                    px(34.0),
                ),
                size(px(clip.duration() as f32 / 96_000.0 * grid.width), px(44.0)),
            );
            prepare_waveform_geometry(
                &source,
                &WaveformMapping::Clip {
                    clip: clip.clone(),
                    timeline_origin: start,
                    gain: None,
                    fade: Some(clip.retained_fade()),
                },
                start,
                end,
                extent,
                32,
                Some(grid),
            )
            .unwrap()
        };
        let baseline = draw(clip);
        for lower_fade in [false, true] {
            for (edge, frame) in [(TrimEdge::Left, 5_001), (TrimEdge::Right, 86_117)] {
                let cropped = if lower_fade {
                    clip.preview_fade_edge_trim(edge, frame, 96_000, editor.project().tempo_map())
                        .unwrap()
                } else {
                    clip.preview_edge_trim(edge, frame, 96_000).unwrap()
                };
                let after = draw(&cropped);
                for channel in 0..2 {
                    let interior = |g: &WaveformGeometry| {
                        g.channels[channel]
                            .outline
                            .iter()
                            .filter(|p| f32::from(p.x) > 300.0 && f32::from(p.x) < 700.0)
                            .copied()
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(interior(&after), interior(&baseline));
                }
            }
        }
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn geometry_cache_separates_vertical_amplitude_scales() {
    let peak = CachedPeak {
        min: -0.1,
        max: 0.1,
    };
    let source = Arc::new(CachedWaveform::from_peaks(100, vec![[peak, peak]; 100]).unwrap());
    let extent = bounds();
    let mapping = WaveformMapping::Source { timeline_origin: 0 };
    let cache = RefCell::new(WaveformGeometryCache::default());
    let get = |decibels| {
        cached_waveform_geometry(
            &cache,
            &source,
            &WaveformGeometryRequest {
                mapping: &mapping,
                amplitude_scale: WaveformAmplitudeScale::from_decibels(decibels),
                visible_start: 0,
                visible_end: 100,
                bounds: extent,
                scale: 1.0,
                columns: 100,
                grid: None,
            },
        )
        .unwrap()
        .0
    };
    let linear = get(false);
    let decibel = get(true);
    assert!(!Arc::ptr_eq(&linear, &decibel));
    assert!(Arc::ptr_eq(&linear, &get(false)));
    assert!(Arc::ptr_eq(&decibel, &get(true)));
    assert!(decibel.channels[0].outline[0].y < linear.channels[0].outline[0].y);
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn geometry_cache_reuses_only_matching_source_mapping_and_bounds() {
    let source = waveform();
    let extent = bounds();
    let cache_key = key(0, extent);
    let geometry = geometry(&source, 0, extent);
    let mut cache = WaveformGeometryCache::default();
    cache.insert(
        cache_key.clone(),
        Arc::clone(&source),
        Arc::clone(&geometry),
        32,
    );
    assert!(Arc::ptr_eq(
        &cache.get(&cache_key, &source).expect("cache hit"),
        &geometry,
    ));
    assert!(
        Arc::strong_count(&source) >= 2,
        "cache retains source identity"
    );

    let resized = Bounds::new(extent.origin, size(px(1201.0), extent.size.height));
    assert!(cache.get(&key(0, resized), &source).is_none());
    let scrolled = Bounds::new(
        point(extent.origin.x, extent.origin.y + px(44.0)),
        extent.size,
    );
    assert!(cache.get(&key(0, scrolled), &source).is_none());
    let scale_changed = waveform_geometry_key(
        &WaveformMapping::Source { timeline_origin: 0 },
        0,
        96_000,
        extent,
        1.0,
    );
    assert!(cache.get(&scale_changed, &source).is_none());
    assert!(cache.get(&key(1, extent), &source).is_none());
    let different_source = waveform();
    assert!(cache.get(&cache_key, &different_source).is_none());
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn coarse_absence_counts_survive_shared_signature_eviction_and_clear() {
    let source = waveform();
    let geometry = geometry(&source, 0, bounds());
    let first = coarse_key(1, 0);
    let second = coarse_key(1, 256);
    let index_key = coarse_entry_key(&first, &source).unwrap();
    let mut cache = WaveformGeometryCache::default();
    for key in [first.clone(), second.clone()] {
        cache.insert(key, Arc::clone(&source), Arc::clone(&geometry), 1);
    }
    assert_eq!(cache.coarse_entry_counts.get(&index_key), Some(&2));

    for clip_id in 2..MAX_CACHED_WAVEFORM_ENTRIES as u64 {
        cache.insert(
            coarse_key(clip_id, clip_id * 256),
            Arc::clone(&source),
            Arc::clone(&geometry),
            1,
        );
    }
    cache.insert(
        coarse_key(10_000, 0),
        Arc::clone(&source),
        Arc::clone(&geometry),
        1,
    );
    assert_eq!(cache.coarse_entry_counts.get(&index_key), Some(&1));
    assert!(cache.get(&second, &source).is_some());
    assert_eq!(cache.coarse_entry_counts.get(&index_key), Some(&1));

    for clip_id in 20_000..20_000 + MAX_CACHED_WAVEFORM_ENTRIES as u64 {
        cache.insert(
            coarse_key(clip_id, clip_id * 256),
            Arc::clone(&source),
            Arc::clone(&geometry),
            1,
        );
    }
    assert!(!cache.coarse_entry_counts.contains_key(&index_key));
    assert!(cache.get(&second, &source).is_none());

    cache.clear();
    assert!(cache.coarse_entry_counts.is_empty());
    assert!(cache.entries.is_empty());
    assert_eq!(cache.columns, 0);
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn clip_detail_gain_changes_cached_geometry_and_unity_reuses_it() {
    let source = waveform();
    let cache = RefCell::new(WaveformGeometryCache::default());
    let neutral = WaveformMapping::GainedSource {
        timeline_origin: 0,
        gain: hex_project::ClipGain::UNITY.linear(),
        detail: false,
    };
    let quiet = WaveformMapping::GainedSource {
        timeline_origin: 0,
        gain: hex_project::ClipGain::new(-6_000).unwrap().linear(),
        detail: false,
    };
    let detailed = WaveformMapping::GainedSource {
        timeline_origin: 0,
        gain: hex_project::ClipGain::UNITY.linear(),
        detail: true,
    };
    let geometry = |mapping| {
        cached_waveform_geometry(
            &cache,
            &source,
            &WaveformGeometryRequest {
                amplitude_scale: crate::editor::waveform::WaveformAmplitudeScale::from_decibels(
                    false,
                ),
                mapping,
                visible_start: 0,
                visible_end: 96_000,
                bounds: bounds(),
                scale: 2.0,
                grid: None,
                columns: 32,
            },
        )
        .unwrap()
        .0
    };
    let first = geometry(&neutral);
    let changed = geometry(&quiet);
    assert!(!Arc::ptr_eq(&first, &changed));
    assert!(Arc::ptr_eq(&first, &geometry(&neutral)));
    assert!(!Arc::ptr_eq(&first, &geometry(&detailed)));
    assert_eq!(cache.borrow().entries.len(), 3);
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn geometry_cache_holds_twelve_visible_clips_and_clip_detail() {
    let source = waveform();
    let extent = bounds();
    let mut cache = WaveformGeometryCache::default();
    for index in 0..13_u64 {
        let origin = index * 96_000;
        let cache_key = key(origin, extent);
        cache.insert(
            cache_key,
            Arc::clone(&source),
            geometry(&source, origin, extent),
            4_000,
        );
    }
    assert_eq!(cache.entries.len(), 13);
    assert_eq!(cache.columns, 52_000);
    for index in 0..13_u64 {
        assert!(cache.get(&key(index * 96_000, extent), &source).is_some());
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn dense_visible_working_set_survives_repeated_selection_paints() {
    let source = waveform();
    let cache = RefCell::new(WaveformGeometryCache::default());
    // The unchanged native fixture paints seven rows of 50 small audio
    // clips: 350 entries, far below the existing aggregate column budget.
    let mut retained = Vec::new();
    for index in 0..350_u64 {
        let origin = index * 96_000;
        let mapping = WaveformMapping::Source {
            timeline_origin: origin,
        };
        let extent = Bounds::new(
            point(
                px((index % 50) as f32 * 14.0),
                px((index / 50) as f32 * 44.0),
            ),
            size(px(9.3), px(44.0)),
        );
        let request = WaveformGeometryRequest {
            amplitude_scale: crate::editor::waveform::WaveformAmplitudeScale::from_decibels(false),
            mapping: &mapping,
            visible_start: origin,
            visible_end: origin + 96_000,
            bounds: extent,
            scale: 2.0,
            grid: None,
            columns: 19,
        };
        retained.push((
            origin,
            extent,
            cached_waveform_geometry(&cache, &source, &request)
                .unwrap()
                .0,
        ));
    }
    for (origin, extent, original) in retained {
        let mapping = WaveformMapping::Source {
            timeline_origin: origin,
        };
        let request = WaveformGeometryRequest {
            amplitude_scale: crate::editor::waveform::WaveformAmplitudeScale::from_decibels(false),
            mapping: &mapping,
            visible_start: origin,
            visible_end: origin + 96_000,
            bounds: extent,
            scale: 2.0,
            grid: None,
            columns: 19,
        };
        let repeated = cached_waveform_geometry(&cache, &source, &request)
            .unwrap()
            .0;
        assert!(
            Arc::ptr_eq(&original, &repeated),
            "visible clip geometry was rebuilt at {origin}"
        );
    }
    assert_eq!(cache.borrow().entries.len(), 350);
    assert_eq!(cache.borrow().columns, 350 * 19);
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn geometry_cache_evicts_by_entry_and_column_limits() {
    let source = waveform();
    let extent = bounds();
    let path_geometry = geometry(&source, 0, extent);
    let mut entry_bounded = WaveformGeometryCache::default();
    for index in 0..(MAX_CACHED_WAVEFORM_ENTRIES + 1) as u64 {
        entry_bounded.insert(
            key(index, extent),
            Arc::clone(&source),
            Arc::clone(&path_geometry),
            1,
        );
    }
    assert_eq!(entry_bounded.entries.len(), MAX_CACHED_WAVEFORM_ENTRIES);
    assert!(entry_bounded.get(&key(0, extent), &source).is_none());
    assert_eq!(entry_bounded.columns, MAX_CACHED_WAVEFORM_ENTRIES);

    let mut column_bounded = WaveformGeometryCache::default();
    for index in 0..17_u64 {
        column_bounded.insert(
            key(index, extent),
            Arc::clone(&source),
            Arc::clone(&path_geometry),
            MAX_WAVEFORM_COLUMNS_FOR_TEST,
        );
    }
    assert_eq!(column_bounded.entries.len(), 16);
    assert_eq!(column_bounded.columns, MAX_CACHED_WAVEFORM_COLUMNS);
    assert!(column_bounded.get(&key(0, extent), &source).is_none());
}

#[test]
fn indexed_lru_matches_reference_eviction_order_invalidation_and_column_budget() {
    let source = waveform();
    let other_source = waveform();
    let extent = bounds();
    let geom_a = geometry(&source, 0, extent);
    let geom_b = geometry(&source, 1, extent);

    // Step-by-step oracle comparing WaveformGeometryCache against a VecDeque LRU.
    let mut cache = WaveformGeometryCache::default();
    let mut oracle: VecDeque<(WaveformGeometryKey, Arc<CachedWaveform>, usize)> = VecDeque::new();
    let mut oracle_columns = 0usize;

    let check_invariants =
        |cache: &WaveformGeometryCache,
         oracle: &VecDeque<(WaveformGeometryKey, Arc<CachedWaveform>, usize)>,
         oracle_columns: usize| {
            assert_eq!(cache.entries.len(), oracle.len());
            assert_eq!(cache.columns, oracle_columns);
            let actual_order: Vec<_> = cache
                .entries
                .iter()
                .map(|entry| (&entry.key, Arc::as_ptr(&entry.source), entry.columns))
                .collect();
            let expected_order: Vec<_> = oracle
                .iter()
                .map(|(k, s, c)| (k, Arc::as_ptr(s), *c))
                .collect();
            assert_eq!(actual_order, expected_order);
        };

    // 1. Insert 600 entries (> MAX_CACHED_WAVEFORM_ENTRIES = 512) with periodic gets & invalidations.
    for i in 0..600_u64 {
        let k = if i % 3 == 0 {
            coarse_key(i, i * 128)
        } else {
            key(i, extent)
        };
        let s = if i % 5 == 0 {
            Arc::clone(&other_source)
        } else {
            Arc::clone(&source)
        };
        let g = if i % 2 == 0 {
            Arc::clone(&geom_a)
        } else {
            Arc::clone(&geom_b)
        };
        let cols = 64 + ((i as usize % 7) * 32);

        cache.insert(k.clone(), Arc::clone(&s), g, cols);
        while oracle.len() >= MAX_CACHED_WAVEFORM_ENTRIES
            || oracle_columns.saturating_add(cols) > MAX_CACHED_WAVEFORM_COLUMNS
        {
            if let Some((_, _, old_cols)) = oracle.pop_front() {
                oracle_columns -= old_cols;
            }
        }
        oracle_columns += cols;
        oracle.push_back((k, s, cols));

        // Promote oldest, middle, and newest entries periodically.
        if i % 17 == 5 && !oracle.is_empty() {
            for probe_idx in [0, oracle.len() / 2, oracle.len() - 1] {
                let (pk, ps, _) = oracle[probe_idx].clone();
                assert!(cache.get(&pk, &ps).is_some());
                let pos = oracle
                    .iter()
                    .position(|(ok, os, _)| ok == &pk && Arc::ptr_eq(os, &ps))
                    .unwrap();
                let moved = oracle.remove(pos).unwrap();
                oracle.push_back(moved);
            }
        }
        // Invalidate head/middle/tail entries periodically.
        if i % 29 == 11 && oracle.len() >= 3 {
            let (ik, isrc, _) = oracle[oracle.len() / 2].clone();
            cache.invalidate(&ik, &isrc);
            let pos = oracle
                .iter()
                .position(|(ok, os, _)| ok == &ik && Arc::ptr_eq(os, &isrc))
                .unwrap();
            let (_, _, removed_cols) = oracle.remove(pos).unwrap();
            oracle_columns -= removed_cols;
            assert!(cache.get(&ik, &isrc).is_none());
        }
        // Miss lookup must not disturb LRU order.
        assert!(cache.get(&key(999_999 + i, extent), &source).is_none());
        check_invariants(&cache, &oracle, oracle_columns);
    }

    // 2. Column-budget eviction strictly enforces MAX_CACHED_WAVEFORM_COLUMNS (65,536).
    let big_cols = MAX_CACHED_WAVEFORM_COLUMNS / 4; // 16,384
    for i in 0..6_u64 {
        let k = key(10_000 + i, extent);
        cache.insert(
            k.clone(),
            Arc::clone(&source),
            Arc::clone(&geom_a),
            big_cols,
        );
        while oracle.len() >= MAX_CACHED_WAVEFORM_ENTRIES
            || oracle_columns.saturating_add(big_cols) > MAX_CACHED_WAVEFORM_COLUMNS
        {
            if let Some((_, _, old_cols)) = oracle.pop_front() {
                oracle_columns -= old_cols;
            }
        }
        oracle_columns += big_cols;
        oracle.push_back((k, Arc::clone(&source), big_cols));
        check_invariants(&cache, &oracle, oracle_columns);
    }
    assert_eq!(cache.entries.len(), 4);
    assert_eq!(cache.columns, MAX_CACHED_WAVEFORM_COLUMNS);

    // Promoting the oldest of the 4 entries protects it from the next eviction.
    let promoted = key(10_002, extent);
    assert!(cache.get(&promoted, &source).is_some());
    cache.insert(
        key(10_006, extent),
        Arc::clone(&source),
        Arc::clone(&geom_a),
        big_cols,
    );
    assert!(cache.get(&promoted, &source).is_some());
    assert!(cache.get(&key(10_003, extent), &source).is_none());
}

#[test]
fn indexed_lru_handles_hash_collisions_and_dense_512_entry_lookups() {
    let source = waveform();
    let extent = bounds();
    let geom_a = geometry(&source, 0, extent);
    let geom_b = geometry(&source, 1, extent);
    let geom_c = geometry(&source, 2, extent);

    // Forced hash collision in GeometryLru: all three share the same bucket hash.
    let mut lru = GeometryLru::default();
    let k0 = key(100, extent);
    let k1 = key(101, extent);
    let k2 = key(102, extent);
    let forced_hash = 0xdead_beef_cafe_babe_u64;
    for (k, g) in [
        (k0.clone(), Arc::clone(&geom_a)),
        (k1.clone(), Arc::clone(&geom_b)),
        (k2.clone(), Arc::clone(&geom_c)),
    ] {
        lru.push_back_with_hash(
            WaveformGeometryEntry {
                key: k,
                source: Arc::clone(&source),
                geometry: g,
                columns: 16,
            },
            forced_hash,
        );
    }
    assert_eq!(lru.len(), 3);
    // Exact match distinguishes colliding keys without false hits.
    let s1 = lru.find_slot_with_hash(forced_hash, &k1, &source).unwrap();
    assert!(Arc::ptr_eq(&lru.geometry_at(s1).unwrap(), &geom_b));
    lru.move_to_back(s1);
    assert!(
        lru.find_slot_with_hash(forced_hash, &key(999, extent), &source)
            .is_none()
    );
    // Removing middle & head shrinks Many -> One -> removed cleanly.
    let removed_mid = lru.remove_slot(s1).unwrap();
    assert_eq!(removed_mid.key, k1);
    let popped = lru.pop_front().unwrap();
    assert_eq!(popped.key, k0);
    assert!(matches!(
        lru.index.get(&forced_hash),
        Some(SlotBucket::One(_))
    ));
    let last = lru.pop_front().unwrap();
    assert_eq!(last.key, k2);
    assert!(lru.is_empty());
    assert!(lru.index.is_empty());

    // Dense 512-entry Arrangement working set: worst-case LRU-head lookup pattern
    // (10,240 hits across 512 entries) completes in O(1) per lookup with 100% hit rate.
    let mut dense = WaveformGeometryCache::default();
    let keys: Vec<_> = (0..MAX_CACHED_WAVEFORM_ENTRIES as u64)
        .map(|i| coarse_key(i, i * 256))
        .collect();
    for k in &keys {
        dense.insert(k.clone(), Arc::clone(&source), Arc::clone(&geom_a), 64);
    }
    assert_eq!(dense.entries.len(), MAX_CACHED_WAVEFORM_ENTRIES);
    assert_eq!(dense.columns, MAX_CACHED_WAVEFORM_ENTRIES * 64);
    // Every entry hashes into its own single-slot bucket (no bucket bloat).
    assert_eq!(dense.entries.index.len(), MAX_CACHED_WAVEFORM_ENTRIES);
    let started = Instant::now();
    for _round in 0..20 {
        for k in &keys {
            let hit = dense.get(k, &source).expect("dense 512-entry hit");
            assert!(Arc::ptr_eq(&hit, &geom_a));
        }
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "10,240 indexed LRU lookups on 512 entries took {elapsed:?}"
    );
    // Accessing all 512 keys in order preserves exact 0..512 LRU-to-MRU order.
    let final_order: Vec<_> = dense
        .entries
        .iter()
        .map(|entry| entry.key.clone())
        .collect();
    assert_eq!(final_order, keys);
}

#[cfg(all(test, not(feature = "ui-test-support")))]
const MAX_WAVEFORM_COLUMNS_FOR_TEST: usize = MAX_CACHED_WAVEFORM_COLUMNS / 16;

#[test]
#[ignore = "manual release profile of waveform geometry preparation reuse"]
fn waveform_geometry_preparation_profile() {
    let source = waveform();
    let extent = bounds();
    let mapping = WaveformMapping::Source { timeline_origin: 0 };
    let visible_start = 0;
    let visible_end = source.source_frames() as u64;
    let columns = waveform_column_count(f32::from(extent.size.width), 2.0);
    let cache = RefCell::new(WaveformGeometryCache::default());
    let request = WaveformGeometryRequest {
        amplitude_scale: crate::editor::waveform::WaveformAmplitudeScale::from_decibels(false),
        mapping: &mapping,
        visible_start,
        visible_end,
        bounds: extent,
        scale: 2.0,
        columns,
        grid: None,
    };
    let rounds = 20;

    let sampled = aggregate_peaks(&source, 0, source.source_frames())
        .expect("fixture extrema survive cache preparation");
    assert!(sampled.iter().any(|peak| peak.min < -0.9 && peak.max > 0.7));
    let digest = source
        .bins()
        .iter()
        .step_by(127)
        .fold(0_u64, |digest, pair| {
            digest.rotate_left(7)
                ^ u64::from(pair[0].min.to_bits())
                ^ u64::from(pair[0].max.to_bits()).rotate_left(17)
        });

    for _ in 0..2 {
        black_box(prepare_waveform_geometry(
            &source,
            &mapping,
            visible_start,
            visible_end,
            extent,
            columns,
            None,
        ));
    }
    let uncached_started = Instant::now();
    for _ in 0..rounds {
        black_box(prepare_waveform_geometry(
            &source,
            &mapping,
            visible_start,
            visible_end,
            extent,
            columns,
            None,
        ));
    }
    let uncached_elapsed = uncached_started.elapsed();

    let warmed = cached_waveform_geometry(&cache, &source, &request)
        .expect("initial cached preparation")
        .0;
    let cached_started = Instant::now();
    for _ in 0..rounds {
        let geometry = cached_waveform_geometry(&cache, &source, &request)
            .expect("warm cache hit")
            .0;
        black_box(Arc::clone(&geometry));
        assert!(Arc::ptr_eq(&geometry, &warmed));
    }
    let cached_elapsed = cached_started.elapsed();
    eprintln!(
        "waveform geometry prep: source=96000 frames/4096 bins, width=1200px, scale=2, columns={columns}, rounds={rounds}, uncached={uncached_elapsed:?}, warm_lookup={cached_elapsed:?}, extrema={sampled:?}, digest={digest:016x}"
    );
}

#[test]
fn arrangement_clip_and_prepared_exact_paint_overrange_markers_only_on_affected_channel() {
    use hex_project::{ClipFade, ClipGain, Command, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "clip".into(),
            path: "clip.wav".into(),
            sample_rate: 48_000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let clip = editor.project().clip(clip).unwrap().clone();
    // Left channel has an over-range positive peak at frame 6; right channel stays at exact -1.0.
    let mut samples = vec![[0.5, -1.0]; 8];
    samples[0] = [1.5, -0.5]; // Silenced by fade-in at frame 0.
    samples[6] = [1.5, -1.0]; // Active at frame 6: L+ exceeds 1.0, R stays exact -1.0.
    let audio = hex_engine::Audio::new(48_000, samples).unwrap();
    let source = Arc::new(hex_media::waveform::summarize(&audio, 8).unwrap());
    let fade = ClipFade::new(8, 0, 4, 0).unwrap();

    let cache = std::rc::Rc::new(std::cell::RefCell::new(WaveformGeometryCache::default()));
    let mapping = WaveformMapping::Clip {
        clip: clip.clone(),
        timeline_origin: 0,
        gain: None,
        fade: Some(fade),
    };
    let request = WaveformGeometryRequest {
        mapping: &mapping,
        amplitude_scale: WaveformAmplitudeScale::Linear,
        visible_start: 0,
        visible_end: 8,
        bounds: bounds(),
        scale: 1.0,
        columns: 4,
        grid: None,
    };
    let _ = cached_waveform_geometry(&cache, &source, &request).unwrap();
    let mut ready = false;
    for _ in 0..200 {
        cache.borrow_mut().poll_overrange();
        let (geom, _) = cached_waveform_geometry(&cache, &source, &request).unwrap();
        if matches!(geom.overrange, WaveformOverrange::Ready(_)) {
            let channels = geom.display.channels();
            assert_eq!(channels.len(), 2);
            assert!(
                channels[0].has_overrange_markers(),
                "left channel over-range column must carry markers"
            );
            assert!(
                !channels[1].has_overrange_markers(),
                "exact -1.0 right channel must keep no-marker fast path"
            );
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(ready, "clip overrange worker must complete");

    // Held gain attenuation pulls the left channel back to <= 1.0: both channels lose markers!
    let attenuated = WaveformMapping::Clip {
        clip: clip.clone(),
        timeline_origin: 0,
        gain: Some(0.5),
        fade: Some(fade),
    };
    let req_att = WaveformGeometryRequest {
        mapping: &attenuated,
        ..request
    };
    let _ = cached_waveform_geometry(&cache, &source, &req_att).unwrap();
    let mut ready_att = false;
    for _ in 0..200 {
        cache.borrow_mut().poll_overrange();
        let (geom, _) = cached_waveform_geometry(&cache, &source, &req_att).unwrap();
        if matches!(geom.overrange, WaveformOverrange::Ready(_)) {
            let channels = geom.display.channels();
            assert!(!channels[0].has_overrange_markers());
            assert!(!channels[1].has_overrange_markers());
            ready_att = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(ready_att, "held-gain overrange worker must complete");

    // PreparedExact inline close-zoom paints markers on affected channel with gain applied once.
    let prepared = hex_session::service::AudioClipWaveform {
        waveform: Arc::clone(&source),
        offset: 0,
        length: 8,
        event_source: None,
        event_sources: None,
        preview_root: None,
        neutral_musical: None,
    };
    let exact = PreparedExact::for_clip(
        editor.project(),
        &clip,
        &prepared,
        None,
        None,
        ClipGain::UNITY,
    )
    .expect("prepared exact");
    let exact_mapping = WaveformMapping::PreparedExact(exact);
    let exact_req = WaveformGeometryRequest {
        mapping: &exact_mapping,
        ..request
    };
    let (geom, _) = cached_waveform_geometry(&cache, &source, &exact_req).unwrap();
    assert!(matches!(geom.overrange, WaveformOverrange::Ready(_)));
    let channels = geom.display.channels();
    assert!(channels[0].has_overrange_markers());
    assert!(!channels[1].has_overrange_markers());
}
