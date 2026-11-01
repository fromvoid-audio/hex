//! Visible Arrangement note intervals for one MIDI clip, derived from the
//! canonical playback occurrence plan (`MidiSourcePlayback::note_occurrences`),
//! the same plan MIDI playback compiles. Display-only: nothing here edits or
//! schedules notes, and no sampling is applied, so every visible loop repeat
//! draws its notes where playback sounds them.
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
use hex_project::{BeatTime, ClipId, Invalid, MidiClip, Project};
use std::{collections::HashMap, rc::Rc, sync::Arc};

/// Per-clip visible-occurrence walk bound (the admission limit).
pub(super) const MAX_VISIBLE_OCCURRENCES: usize = 65_536;

pub(super) struct VisibleNotes {
    /// `(frame_start, frame_gate_end, pitch)`, sorted by pitch then onset.
    pub(super) intervals: Vec<(u64, u64, u8)>,
    /// Row fit over every pitch that sounds anywhere in the clip, so the
    /// vertical zoom does not change while scrolling.
    pub(super) fit: PitchFit,
}

/// Exact note intervals intersecting `[visible_start, visible_end)` frames.
/// `merge_frames` (one display pixel in frames; `0` disables merging) joins
/// same-pitch intervals separated by less than that gap.
pub(super) fn visible_notes(
    project: &Project,
    clip: &MidiClip,
    visible_start: u64,
    visible_end: u64,
    merge_frames: u64,
) -> Result<VisibleNotes, Invalid> {
    let tempo = project.tempo_map();
    let source = clip.source_playback()?;
    let clip_start = clip.start().raw();
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
    let mut pitches = Vec::new();
    for note in clip.notes() {
        let plan = source.note_occurrences(note.start(), note.duration())?;
        let count = plan.count();
        if count == 0 {
            continue;
        }
        pitches.push(note.pitch());
        // Occurrences of one note never overlap and ascend in timeline order,
        // so the first one ending after the window start is a partition point.
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
                intervals.push((start, end, note.pitch()));
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
    Ok(VisibleNotes {
        intervals,
        fit: PitchFit::from_pitches(pitches),
    })
}

/// Visible frames and merge gap one cached result was computed for.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Window {
    start: u64,
    end: u64,
    merge_frames: u64,
}

/// Last visible notes per clip for one immutable project snapshot.
///
/// Holding the snapshot `Arc` keeps cached clips immutable and rules out a
/// reused address matching a different project. Entries are bounded by the
/// snapshot's clip count (one per clip, replaced when its window changes) and
/// are all dropped when a different snapshot is drawn. A refused walk is
/// cached as `None` so an over-limit clip is not re-walked every frame.
#[derive(Default)]
pub(super) struct VisibleNotesCache {
    project: Option<Arc<Project>>,
    entries: HashMap<ClipId, (Window, Option<Rc<VisibleNotes>>)>,
}

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
        if let Some((cached, notes)) = self.entries.get(&clip.id())
            && *cached == window
        {
            return notes.clone();
        }
        let notes = visible_notes(project, clip, visible_start, visible_end, merge_frames)
            .ok()
            .map(Rc::new);
        self.entries.insert(clip.id(), (window, notes.clone()));
        notes
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

    /// Rendered regression for the user report: the resting Arrangement clip
    /// paints every repeat's notes at playback positions and no count badge.
    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn rendered_long_loop_paints_every_repeat_without_a_sample_badge(
        cx: &mut gpui::TestAppContext,
    ) {
        use std::sync::Arc;
        let (editor, id) = looped_clip();
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
        view.update(&mut visual, |view, cx| {
            // The fixture timeline is shorter than this clip; extend it so the
            // whole loop is visible.
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
                start,
                length: end - start,
            });
            cx.notify();
        });
        visual.run_until_parked();
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
    }
}
