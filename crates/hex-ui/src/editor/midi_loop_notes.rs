//! Visible Arrangement note intervals for one MIDI clip, derived from the
//! canonical playback occurrence plan (`MidiSourcePlayback::note_occurrences`),
//! the same plan MIDI playback compiles. Display-only: nothing here edits or
//! schedules notes, and no sampling is applied, so every visible loop repeat
//! draws its notes where playback sounds them. The resting clip, its
//! time-move fragments, drag/drop ghosts and the edge-trim preview all draw
//! through [`visible_occurrences`]; a trim supplies its previewed playback.
//!
//! Work is bounded by the visible window: each note locates its first visible
//! occurrence by binary search over the constant-time plan, then walks only
//! the visible repeats. `MAX_VISIBLE_OCCURRENCES` matches the project-wide
//! occurrence admission limit, so an admitted clip is never truncated. Notes
//! of one pitch that touch within one display pixel are merged, which bounds
//! painted quads by visible pixels and pitches rather than by repeats.
//! `VisibleNotesCache` reuses a clip's result while its project snapshot and
//! visible window are unchanged, so resting repaints skip the walk.
use super::midi_thumbnail::PitchFit;
use hex_project::{BeatTime, ClipId, Invalid, MidiClip, MidiNoteOccurrencePlan, Project, TempoMap};
use std::{collections::HashMap, rc::Rc, sync::Arc};

/// Per-clip visible-occurrence walk bound (the admission limit).
pub(super) const MAX_VISIBLE_OCCURRENCES: usize = 65_536;

/// Cached windows per clip: the resting body plus the few gesture fragments
/// (time-move residuals, a moved copy, a drop ghost) one frame draws.
const WINDOWS_PER_CLIP: usize = 4;

pub(super) struct VisibleNotes {
    /// `(frame_start, frame_gate_end, pitch)`, sorted by pitch then onset.
    pub(super) intervals: Vec<(u64, u64, u8)>,
    /// Row fit over every pitch that sounds anywhere in the clip, so the
    /// vertical zoom does not change while scrolling.
    pub(super) fit: PitchFit,
}

/// Frames covered by one device pixel of a `pixel_width`-wide body showing
/// `span` frames: the gap below which same-pitch notes merge into one quad.
pub(super) fn merge_frames(span: u64, pixel_width: f32, scale_factor: f32) -> u64 {
    let physical = f64::from(pixel_width * scale_factor.max(1.0));
    if physical >= 1.0 {
        (span as f64 / physical) as u64
    } else {
        span
    }
}

/// Row fit over the pitches that sound under the clip's canonical playback,
/// shared by the resting body and its gesture previews.
pub(super) fn sounding_fit(clip: &MidiClip) -> PitchFit {
    let Ok(source) = clip.source_playback() else {
        return PitchFit::from_pitches([]);
    };
    PitchFit::from_pitches(clip.notes().iter().filter_map(|note| {
        source
            .note_occurrences(note.start(), note.duration())
            .is_ok_and(|plan| plan.count() > 0)
            .then_some(note.pitch())
    }))
}

/// Exact note intervals of the canonical `clip` intersecting
/// `[visible_start, visible_end)` frames. `merge_frames` (one display pixel in
/// frames; `0` disables merging) joins same-pitch intervals separated by less
/// than that gap.
pub(super) fn visible_notes(
    project: &Project,
    clip: &MidiClip,
    visible_start: u64,
    visible_end: u64,
    merge_frames: u64,
) -> Result<VisibleNotes, Invalid> {
    let source = clip.source_playback()?;
    let mut plans = Vec::with_capacity(clip.notes().len());
    for note in clip.notes() {
        plans.push((
            source.note_occurrences(note.start(), note.duration())?,
            note.pitch(),
        ));
    }
    let fit = PitchFit::from_pitches(
        plans
            .iter()
            .filter(|(plan, _)| plan.count() > 0)
            .map(|&(_, pitch)| pitch),
    );
    Ok(VisibleNotes {
        intervals: visible_occurrences(
            project.tempo_map(),
            clip.start(),
            plans,
            visible_start,
            visible_end,
            merge_frames,
        )?,
        fit,
    })
}

/// Visible `(frame_start, frame_gate_end, pitch)` intervals of `plans`, whose
/// occurrences are relative to `timeline_start`, sorted by pitch then onset.
/// Refuses rather than truncates past `MAX_VISIBLE_OCCURRENCES`.
pub(super) fn visible_occurrences(
    tempo: &TempoMap,
    timeline_start: BeatTime,
    plans: impl IntoIterator<Item = (MidiNoteOccurrencePlan, u8)>,
    visible_start: u64,
    visible_end: u64,
    merge_frames: u64,
) -> Result<Vec<(u64, u64, u8)>, Invalid> {
    let clip_start = timeline_start.raw();
    // Clip-local beat window, widened by one raw beat for frame rounding; the
    // exact frame cull below decides membership.
    let local = |frame: u64| -> Result<u64, Invalid> {
        Ok(tempo.frame_to_beat(frame)?.raw().saturating_sub(clip_start))
    };
    let window_start = local(visible_start)?.saturating_sub(1);
    let window_end = local(visible_end)?.saturating_add(1);
    let frame = |local_beat: u64| -> Result<u64, Invalid> {
        let beat = clip_start
            .checked_add(local_beat)
            .ok_or(Invalid("midi note beat overflow"))?;
        tempo.beat_to_frame(BeatTime::from_raw(beat))
    };
    let mut intervals = Vec::new();
    for (plan, pitch) in plans {
        let count = plan.count();
        if count == 0 {
            continue;
        }
        // Occurrences of one note never overlap and ascend in timeline order
        // (also when reversed), so the first one ending after the window
        // start is a partition point. `nth` is constant-time on the plan.
        let end_of = |ordinal: u64| -> Option<u64> {
            let hit = plan.iter().nth(usize::try_from(ordinal).ok()?)?;
            hit.timeline_start()
                .raw()
                .checked_add(hit.gate_duration().raw())
        };
        let (mut low, mut high) = (0_u64, count);
        while low < high {
            let middle = low + (high - low) / 2;
            if end_of(middle).is_some_and(|end| end <= window_start) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let skip = usize::try_from(low).map_err(|_| Invalid("midi occurrence index overflow"))?;
        for hit in plan.iter().skip(skip) {
            let start = hit.timeline_start().raw();
            if start >= window_end {
                break;
            }
            if intervals.len() >= MAX_VISIBLE_OCCURRENCES {
                return Err(Invalid(
                    "visible midi occurrences exceed the admission limit",
                ));
            }
            let end = start
                .checked_add(hit.gate_duration().raw())
                .ok_or(Invalid("midi note gate overflow"))?;
            let (start, end) = (frame(start)?, frame(end)?);
            if start < visible_end && end > visible_start && end > start {
                intervals.push((start, end, pitch));
            }
        }
    }
    intervals.sort_unstable_by_key(|&(start, end, pitch)| (pitch, start, end));
    if merge_frames > 0 {
        let mut merged: Vec<(u64, u64, u8)> = Vec::with_capacity(intervals.len());
        for (start, end, pitch) in intervals {
            match merged.last_mut() {
                Some(last) if last.2 == pitch && start < last.1.saturating_add(merge_frames) => {
                    last.1 = last.1.max(end);
                }
                _ => merged.push((start, end, pitch)),
            }
        }
        intervals = merged;
    }
    Ok(intervals)
}

/// Visible frames and merge gap one cached result was computed for.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Window {
    start: u64,
    end: u64,
    merge_frames: u64,
}

/// Recent visible notes per clip for one immutable project snapshot.
///
/// Holding the snapshot `Arc` keeps cached clips immutable and rules out a
/// reused address matching a different project. Each clip keeps at most
/// `WINDOWS_PER_CLIP` windows, most recent first, so a gesture preview does
/// not evict the resting body; entries are bounded by the snapshot's clip
/// count and are all dropped when a different snapshot is drawn. A refused
/// walk is cached as `None` so an over-limit clip is not re-walked every frame.
#[derive(Default)]
pub(super) struct VisibleNotesCache {
    project: Option<Arc<Project>>,
    entries: HashMap<ClipId, Vec<CachedWindow>>,
}

/// One cached window and its walk; `None` when the walk was refused.
type CachedWindow = (Window, Option<Rc<VisibleNotes>>);

impl VisibleNotesCache {
    /// `visible_notes` for `clip`, reused when the snapshot and window match.
    pub(super) fn get(
        &mut self,
        project: &Arc<Project>,
        clip: &MidiClip,
        visible_start: u64,
        visible_end: u64,
        merge_frames: u64,
    ) -> Option<Rc<VisibleNotes>> {
        if !self
            .project
            .as_ref()
            .is_some_and(|cached| Arc::ptr_eq(cached, project))
        {
            self.entries.clear();
            self.project = Some(Arc::clone(project));
        }
        let window = Window {
            start: visible_start,
            end: visible_end,
            merge_frames,
        };
        let windows = self.entries.entry(clip.id()).or_default();
        if let Some(index) = windows.iter().position(|(cached, _)| *cached == window) {
            let hit = windows.remove(index);
            let notes = hit.1.clone();
            windows.insert(0, hit);
            return notes;
        }
        let notes = visible_notes(project, clip, visible_start, visible_end, merge_frames)
            .ok()
            .map(Rc::new);
        windows.truncate(WINDOWS_PER_CLIP - 1);
        windows.insert(0, (window, notes.clone()));
        notes
    }
}

impl super::EditorView {
    /// Shapes (fractions of `[start, end)`) and row fit of the canonical
    /// `clip`'s notes in those frames, through the per-clip cache. A
    /// re-entrant build computes without the cache; a refused walk draws none.
    pub(super) fn loop_note_shapes(
        &self,
        project: &Arc<Project>,
        clip: &MidiClip,
        start: u64,
        end: u64,
        merge_frames: u64,
    ) -> (Vec<(f32, f32, u8)>, PitchFit) {
        let visible = match self.midi_visible_notes.try_borrow_mut() {
            Ok(mut cache) => cache.get(project, clip, start, end, merge_frames),
            Err(_) => visible_notes(project, clip, start, end, merge_frames)
                .ok()
                .map(Rc::new),
        };
        let Some(visible) = visible else {
            return (Vec::new(), sounding_fit(clip));
        };
        let shapes = visible
            .intervals
            .iter()
            .filter_map(|&(note_start, note_end, pitch)| {
                super::midi_thumbnail::note_shape(note_start, note_end, pitch, start, end)
            })
            .collect();
        (shapes, visible.fit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Tempo};

    fn beats(quarters: u64) -> BeatTime {
        BeatTime::from_quarters(quarters).unwrap()
    }

    /// Loop start differs from the clip start, the source has a start offset,
    /// a tempo change lands mid-clip and the last repeat is partial.
    fn looped_clip() -> (Editor, hex_project::ClipId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "loop".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beats(2),
                length: beats(4),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        for (start, pitch) in [(BeatTime::ZERO, 60), (beats(1), 64), (beats(2), 67)] {
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start,
                    duration: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
        // Source starts one beat in, loops [1, 3) and runs 40 repeats plus a
        // half-period tail: 1 + 2 * 40 + 1 beats.
        editor
            .execute(Command::SetMidiClipSource {
                clip,
                source_length: beats(4),
                source_start: beats(1),
                loop_start: beats(1),
                loop_end: beats(3),
                looping: true,
                length: beats(82),
            })
            .unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: beats(30),
                tempo: Tempo::new(90_000_000).unwrap(),
            })
            .unwrap();
        (editor, clip)
    }

    /// Exactly what playback compiles: every occurrence of every note.
    fn playback(project: &Project, clip: &MidiClip) -> Vec<(u64, u64, u8)> {
        let source = clip.source_playback().unwrap();
        let tempo = project.tempo_map();
        let frame = |beat: u64| {
            tempo
                .beat_to_frame(BeatTime::from_raw(clip.start().raw() + beat))
                .unwrap()
        };
        let mut all: Vec<_> = clip
            .notes()
            .iter()
            .flat_map(|note| {
                let plan = source
                    .note_occurrences(note.start(), note.duration())
                    .unwrap();
                plan.iter()
                    .map(|hit| {
                        let start = hit.timeline_start().raw();
                        (
                            frame(start),
                            frame(start + hit.gate_duration().raw()),
                            note.pitch(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        all.sort_unstable_by_key(|&(start, end, pitch)| (pitch, start, end));
        all
    }

    #[test]
    fn every_visible_repeat_draws_its_notes_where_playback_sounds_them() {
        let (editor, id) = looped_clip();
        let project = editor.project();
        let clip = project.midi_clip(id).unwrap();
        let expected = playback(project, clip);
        // Two notes inside the loop over 40+ repeats: far more than the old
        // 32-sample engine preview.
        assert!(expected.len() > 64, "{}", expected.len());
        let tempo = project.tempo_map();
        let start = tempo.beat_to_frame(clip.start()).unwrap();
        let end = tempo
            .beat_to_frame(BeatTime::from_raw(clip.start().raw() + beats(82).raw()))
            .unwrap();
        let whole = visible_notes(project, clip, start, end, 0).unwrap();
        assert_eq!(whole.intervals, expected);
        // A window across the tempo change and into the partial last repeat
        // keeps exactly the intersecting playback occurrences.
        let (from, to) = (
            tempo.beat_to_frame(beats(25)).unwrap(),
            tempo.beat_to_frame(beats(84)).unwrap(),
        );
        let window = visible_notes(project, clip, from, to.min(end), 0).unwrap();
        let culled: Vec<_> = expected
            .iter()
            .copied()
            .filter(|&(s, e, _)| s < to.min(end) && e > from)
            .collect();
        assert_eq!(window.intervals, culled);
        // The note before the source start never sounds and does not widen the fit.
        assert_eq!(whole.fit, PitchFit::from_pitches([64, 67]));
    }

    #[test]
    fn same_pitch_notes_within_one_pixel_merge_into_one_quad() {
        let (editor, id) = looped_clip();
        let project = editor.project();
        let clip = project.midi_clip(id).unwrap();
        let tempo = project.tempo_map();
        let start = tempo.beat_to_frame(clip.start()).unwrap();
        let end = tempo
            .beat_to_frame(BeatTime::from_raw(clip.start().raw() + beats(82).raw()))
            .unwrap();
        let exact = visible_notes(project, clip, start, end, 0).unwrap();
        let coarse = visible_notes(project, clip, start, end, end - start).unwrap();
        assert!(coarse.intervals.len() < exact.intervals.len());
        assert_eq!(coarse.intervals.len(), 2, "one span per sounding pitch");
    }

    #[test]
    fn cache_reuses_only_the_same_snapshot_and_window() {
        let (mut editor, id) = looped_clip();
        let project = Arc::new(editor.project().clone());
        let clip = project.midi_clip(id).unwrap();
        let mut cache = VisibleNotesCache::default();
        let first = cache.get(&project, clip, 0, 400_000, 100).unwrap();
        let again = cache.get(&project, clip, 0, 400_000, 100).unwrap();
        assert!(
            Rc::ptr_eq(&first, &again),
            "resting repaint reuses the walk"
        );
        let scrolled = cache.get(&project, clip, 50_000, 450_000, 100).unwrap();
        assert!(!Rc::ptr_eq(&first, &scrolled));
        assert_eq!(
            scrolled.intervals,
            visible_notes(&project, clip, 50_000, 450_000, 100)
                .unwrap()
                .intervals
        );
        // An edit publishes a new snapshot; the old result must not be reused.
        editor
            .execute(Command::SetTempoPoint {
                beat: beats(4),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        let edited = Arc::new(editor.project().clone());
        let clip = edited.midi_clip(id).unwrap();
        let fresh = cache.get(&edited, clip, 50_000, 450_000, 100).unwrap();
        assert_eq!(
            fresh.intervals,
            visible_notes(&edited, clip, 50_000, 450_000, 100)
                .unwrap()
                .intervals
        );
        assert_ne!(fresh.intervals, scrolled.intervals);
    }

    /// Publish `project` (or keep the current one) with a timeline and
    /// viewport showing `length` frames from `start`; the fixture timeline is
    /// shorter than the looped clip.
    #[cfg(feature = "ui-test-support")]
    fn publish(
        visual: &mut gpui::VisualTestContext,
        view: &gpui::Entity<crate::editor::EditorView>,
        project: Option<Project>,
        start: u64,
        length: u64,
    ) {
        view.update(visual, |view, cx| {
            let old = view.state.take().unwrap();
            let (project, midi_preview) = match project {
                Some(project) => {
                    let (_, preview) = hex_engine::compile::project_with_midi_preview(
                        &project,
                        &Default::default(),
                    )
                    .unwrap();
                    (Arc::new(project), Arc::new(preview))
                }
                None => (Arc::clone(&old.project), Arc::clone(&old.midi_preview)),
            };
            view.state = Some(Arc::new(hex_session::service::ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project,
                waveforms: Default::default(),
                source_overviews: Default::default(),
                audio_clip_waveforms: std::collections::BTreeMap::new(),
                midi_preview,
                gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                timeline_frames: start + length,
                can_undo: false,
                can_redo: false,
                revision: old.revision + 1,
                unsaved: false,
                document_path: None,
            }));
            view.viewport = Some(crate::editor::viewport::Viewport { start, length });
            cx.notify();
        });
        visual.run_until_parked();
    }

    /// Rendered regression for the user report: the resting Arrangement clip
    /// paints every repeat's notes at playback positions and no count badge.
    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn rendered_long_loop_paints_every_repeat_without_a_sample_badge(
        cx: &mut gpui::TestAppContext,
    ) {
        let (mut editor, id) = looped_clip();
        let project = editor.project().clone();
        let clip = project.midi_clip(id).unwrap().clone();
        let expected = playback(&project, &clip);
        let tempo = project.tempo_map();
        let start = tempo.beat_to_frame(clip.start()).unwrap();
        let end = tempo
            .beat_to_frame(BeatTime::from_raw(clip.start().raw() + beats(82).raw()))
            .unwrap();
        let (_, preview) =
            hex_engine::compile::project_with_midi_preview(&project, &Default::default()).unwrap();
        let (handle, view) =
            crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::new(preview));
        let mut visual = gpui::VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(gpui::px(2400.0), gpui::px(800.0)));
        // The fixture timeline is shorter than this clip; publish a snapshot
        // whose timeline shows the whole loop.
        let install = |visual: &mut gpui::VisualTestContext, project: Option<Project>| {
            publish(visual, &view, project, start, end - start)
        };
        install(&mut visual, None);
        let body = visual
            .debug_bounds(Box::leak(
                format!("clip-notes-{}", id.value()).into_boxed_str(),
            ))
            .expect("note body");
        let notes = crate::editor::midi_thumbnail::rendered_notes(&mut visual, id, false);
        assert_eq!(
            notes.len(),
            expected.len(),
            "every repeat is drawn, not a 32-note sample"
        );
        let (left, width) = (f32::from(body.left()), f32::from(body.size.width));
        for (onset, _, _) in &expected {
            let x = left + (onset - start) as f32 / (end - start) as f32 * width;
            assert!(
                notes
                    .iter()
                    .any(|(bounds, _)| (f32::from(bounds.left()) - x).abs() <= 1.5),
                "a note at playback frame {onset} (x {x})"
            );
        }
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("midi-preview-count-{}", id.value()).into_boxed_str(),
                ))
                .is_none(),
            "no sample-count badge"
        );
        // A resting repaint reuses the cached walk; an edit that leaves the
        // visible window unchanged publishes a new snapshot and must repaint
        // from it, not from the cached notes.
        install(&mut visual, None);
        assert_eq!(
            crate::editor::midi_thumbnail::rendered_notes(&mut visual, id, false).len(),
            expected.len()
        );
        editor
            .execute(Command::AddMidiNote {
                clip: id,
                start: BeatTime::from_raw(BeatTime::ONE.raw() * 3 / 2),
                duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
                pitch: 72,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let edited = editor.project().clone();
        let added = playback(&edited, edited.midi_clip(id).unwrap());
        assert!(added.len() > expected.len() + 32, "{}", added.len());
        install(&mut visual, Some(edited));
        assert_eq!(
            crate::editor::midi_thumbnail::rendered_notes(&mut visual, id, false).len(),
            added.len(),
            "the edit's notes paint at every repeat"
        );
    }

    /// #4137: dragging a long loop's right edge previews the shortened clip
    /// with every visible repeat's notes where playback sounds them, not the
    /// 32 engine samples.
    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn rendered_long_loop_trim_preview_paints_every_visible_repeat(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, MouseButton, point, px};
        let (editor, id) = looped_clip();
        let project = editor.project().clone();
        let clip = project.midi_clip(id).unwrap().clone();
        let tempo = project.tempo_map().clone();
        let start = tempo.beat_to_frame(clip.start()).unwrap();
        let end = tempo
            .beat_to_frame(BeatTime::from_raw(clip.start().raw() + beats(82).raw()))
            .unwrap();
        let (_, preview) =
            hex_engine::compile::project_with_midi_preview(&project, &Default::default()).unwrap();
        let (handle, view) =
            crate::editor::clip_drag_ui_test::editor_window(cx, project.clone(), Arc::new(preview));
        let mut visual = gpui::VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(gpui::px(2400.0), gpui::px(800.0)));
        // Leave room right of the clip for its edge handle.
        publish(&mut visual, &view, None, start, (end - start) / 10 * 11);
        let edge = visual
            .debug_bounds(Box::leak(
                format!("midi-clip-trim-right-{}", id.value()).into_boxed_str(),
            ))
            .expect("right MIDI trim edge");
        let down = crate::editor::clip_drag_ui_test::trim_grab_point(edge);
        visual.simulate_mouse_move(down, None, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x - px(12.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        visual.simulate_mouse_move(
            point(down.x - px(400.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        let trim_end = view.read_with(&visual, |view, _| {
            let drag = view
                .active_midi_trim_drag
                .as_ref()
                .expect("right MIDI trim");
            let preview = drag.preview.borrow().expect("trim preview");
            assert_eq!(
                preview.start(),
                clip.start(),
                "a right trim keeps the start"
            );
            BeatTime::from_raw(preview.start().raw() + preview.length().raw())
        });
        let trim_end = tempo.beat_to_frame(trim_end).unwrap();
        assert!(start < trim_end && trim_end < end, "{trim_end}");
        let expected: Vec<_> = playback(&project, &clip)
            .into_iter()
            .filter(|&(onset, _, _)| onset < trim_end)
            .collect();
        assert!(expected.len() > 32, "{}", expected.len());
        let body = visual
            .debug_bounds("midi-trim-preview-notes")
            .expect("trim preview note layer");
        let notes = crate::editor::midi_thumbnail::rendered_note_layer(
            &mut visual,
            "midi-trim-preview-notes",
        );
        assert_eq!(
            notes.len(),
            expected.len(),
            "every visible repeat is drawn, not a 32-note sample"
        );
        let (left, width) = (f32::from(body.left()), f32::from(body.size.width));
        for (onset, _, _) in &expected {
            let x = left + (onset - start) as f32 / (trim_end - start) as f32 * width;
            assert!(
                notes
                    .iter()
                    .any(|(bounds, _)| (f32::from(bounds.left()) - x).abs() <= 1.5),
                "a trim preview note at playback frame {onset} (x {x})"
            );
        }
    }
}
