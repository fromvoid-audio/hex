//! Manual release profile of resting Arrangement repaints over many long
//! looped MIDI clips. Headless CPU only; no native frame-deadline evidence.

use std::sync::Arc;

use gpui::{TestAppContext, VisualTestContext, px, size};
use hex_project::{BeatTime, Command, Created, Editor, Project};

const TRACKS: usize = 12;
const CLIPS_PER_TRACK: u64 = 8;
/// 96 clips x 40 repeats x 16 notes = 61,440 occurrences, just under the
/// project-wide compiled limit of 65,536.
const CLIP_BEATS: u64 = 160;
const LOOP_BEATS: u64 = 4;
const NOTES_PER_LOOP: u64 = 16;

fn beats(quarters: u64) -> BeatTime {
    BeatTime::from_quarters(quarters).unwrap()
}

/// `TRACKS` tracks of `CLIPS_PER_TRACK` back-to-back clips; each loops a
/// `LOOP_BEATS` source of `NOTES_PER_LOOP` sixteenths across `CLIP_BEATS`.
fn project() -> Project {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
    for t in 0..TRACKS {
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: format!("loop {t}"),
            })
            .unwrap()
        else {
            panic!("track")
        };
        for c in 0..CLIPS_PER_TRACK {
            let Created::Clip(clip) = editor
                .execute(Command::PlaceMidiClip {
                    track,
                    start: beats(c * CLIP_BEATS),
                    length: beats(LOOP_BEATS),
                })
                .unwrap()
            else {
                panic!("clip")
            };
            let sixteenth = BeatTime::ONE.raw() / 4;
            for n in 0..NOTES_PER_LOOP {
                editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::from_raw(n * sixteenth),
                        duration: BeatTime::from_raw(sixteenth / 2),
                        pitch: 48 + ((n * 5 + t as u64) % 24) as u8,
                        channel: 0,
                        velocity: 100,
                    })
                    .unwrap();
            }
            editor
                .execute(Command::SetMidiClipSource {
                    clip,
                    source_length: beats(LOOP_BEATS),
                    source_start: BeatTime::ZERO,
                    loop_start: BeatTime::ZERO,
                    loop_end: beats(LOOP_BEATS),
                    looping: true,
                    length: beats(CLIP_BEATS),
                })
                .unwrap();
        }
    }
    editor.project().clone()
}

/// Times `notify` plus a completed repaint of the resting arrangement, at a
/// whole-project zoom and at an eight-bar zoom.
#[gpui::test]
#[ignore = "manual optimized long MIDI loop repaint profile; no native acceptance"]
fn profile_long_midi_loop_repaint(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_MIDI_LOOP_PROFILE_SMOKE").is_some_and(|v| v == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let (warmup, samples) = if smoke { (1, 4) } else { (16, 128) };
    let project = project();
    let tempo = project.tempo_map();
    let end = tempo
        .beat_to_frame(beats(CLIPS_PER_TRACK * CLIP_BEATS))
        .unwrap();
    let eight_bars = tempo.beat_to_frame(beats(32)).unwrap();
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default()).unwrap();
    let (handle, view) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(2400.0), px(1400.0)));
    for (zoom, length) in [("whole", end), ("eight_bars", eight_bars)] {
        view.update(&mut visual, |view, cx| {
            let old = view.state.take().unwrap();
            view.state = Some(Arc::new(hex_session::service::ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project: Arc::clone(&old.project),
                waveforms: Default::default(),
                source_overviews: Default::default(),
                audio_clip_waveforms: std::collections::BTreeMap::new(),
                midi_preview: Arc::clone(&old.midi_preview),
                gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                timeline_frames: end,
                can_undo: false,
                can_redo: false,
                revision: 1,
                unsaved: false,
                document_path: None,
            }));
            view.viewport = Some(crate::editor::viewport::Viewport {
                start: (end - length) / 2,
                length,
            });
            cx.notify();
        });
        visual.run_until_parked();
        let mut timings = Vec::with_capacity(samples);
        for round in 0..warmup + samples {
            let clock = std::time::Instant::now();
            view.update(&mut visual, |_, cx| cx.notify());
            visual.run_until_parked();
            let elapsed = clock.elapsed().as_secs_f64() * 1_000_000.;
            if round >= warmup {
                timings.push(elapsed);
            }
        }
        timings.sort_by(f64::total_cmp);
        let percentile = |p: usize| timings[(timings.len() - 1) * p / 100];
        let scale = visual.update(|w, _| w.scale_factor());
        println!(
            "MIDI_LOOP_PROFILE zoom={zoom} width=2400 height=1400 scale={scale} tracks={TRACKS} clips={} clip_beats={CLIP_BEATS} loop_beats={LOOP_BEATS} notes_per_loop={NOTES_PER_LOOP} occurrences={} warmup={warmup} samples={samples} debug_smoke={smoke} headless_cpu_only=true p50_us={:.1} p95_us={:.1} p99_us={:.1} max_us={:.1}",
            TRACKS as u64 * CLIPS_PER_TRACK,
            TRACKS as u64 * CLIPS_PER_TRACK * CLIP_BEATS / LOOP_BEATS * NOTES_PER_LOOP,
            percentile(50),
            percentile(95),
            percentile(99),
            percentile(100)
        );
    }
}
