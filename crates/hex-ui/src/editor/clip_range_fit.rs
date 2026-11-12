//! Arrangement time selection to clip editor viewport, as in Live.
//!
//! Completing a dragged Arrangement time selection over the clip editor's
//! subject (`detail_clip_id`) fits the clip editor to exactly that range in
//! the editor's own coordinates. The frame range is clamped to the clip's
//! placement. Both endpoints map through the same project laws that place the
//! clip editor's playback line, so selection, line and playback agree:
//! - MIDI: frames to clip-elapsed beats through the tempo map, then through the
//!   source playback (playback offset, loop phase) to source beats.
//! - Audio: `AudioViewProjection::root_position_for_frame`, in the retained
//!   root clock (frames or beats), including warp, repeat phase and tempo. A
//!   looped placement's editor root spans the whole placement, so repeats map
//!   unfolded, exactly where the editor draws them.
//!
//! The MIDI exclusive end maps through elapsed beats exactly; the audio end
//! maps its last covered frame plus one root unit. A MIDI range that crosses a
//! loop wrap or covers more than one cycle, or any range whose mapped end
//! precedes its start (a reversed read), fits the whole clip instead. Only the clip editor viewport
//! changes; zoom-back state, canonical project state and the Arrangement
//! viewport are untouched.
use super::EditorView;
use hex_project::{BeatTime, Clip, MidiClip, TempoMap};

/// Source-beat clip editor range for an Arrangement frame range over a MIDI
/// placement; `None` when the range misses the placement.
pub(super) fn midi_source_range(
    tempo: &TempoMap,
    placement: &MidiClip,
    range: (u64, u64),
) -> Option<(u64, u64)> {
    let start = tempo.beat_to_frame(placement.start()).ok()?;
    let end = tempo
        .beat_to_frame(BeatTime::from_raw(
            placement
                .start()
                .raw()
                .checked_add(placement.arrangement_length().raw())?,
        ))
        .ok()?;
    let first = range.0.max(start);
    let last = range.1.min(end).checked_sub(1)?;
    if last < first {
        return None;
    }
    let elapsed = |frame| {
        super::midi_cue::arrangement_elapsed(
            tempo,
            placement.start(),
            placement.arrangement_length(),
            frame,
        )
    };
    let (a, b) = (elapsed(first)?, elapsed(last)?);
    // The exclusive end in elapsed beats; the placement end is its length.
    let span = if range.1 >= end {
        placement.arrangement_length().raw()
    } else {
        elapsed(range.1)?.raw()
    }
    .checked_sub(a.raw())?;
    let playback = placement
        .source()
        .playback_for_duration(placement.arrangement_length())
        .and_then(|playback| playback.with_playback_offset(placement.playback_offset()))
        .ok()?;
    let whole = (0, placement.source().source_length().raw());
    let (from, to) = (
        playback.source_phase_at_elapsed(a)?.raw(),
        playback.source_phase_at_elapsed(b)?.raw(),
    );
    if to < from || b.raw() - a.raw() > to - from {
        return Some(whole);
    }
    Some((from, from.checked_add(span)?))
}

/// Root-clock clip editor range for an Arrangement frame range over an audio
/// placement; `None` when the range misses the placement or its audible part.
pub(super) fn audio_root_range(
    tempo: &TempoMap,
    clip: &Clip,
    range: (u64, u64),
    whole: (u64, u64),
) -> Option<(u64, u64)> {
    let end = clip.start().checked_add(clip.duration())?;
    let first = range.0.max(clip.start());
    let last = range.1.min(end).checked_sub(1)?;
    if last < first {
        return None;
    }
    let projection = hex_project::AudioViewProjection::new(clip, tempo).ok()?;
    let from = projection.root_position_for_frame(first).ok()??;
    let to = projection.root_position_for_frame(last).ok()??;
    if to < from {
        return Some(whole);
    }
    Some((from, to.checked_add(1)?))
}

impl EditorView {
    /// Whole-clip selection fits the visible detail pane once. The user can
    /// still navigate freely until their next explicit clip selection.
    pub(super) fn fit_clip_editor_to_selected_clip(&mut self) {
        if !self.clip_view_open || self.device_panel_active || self.selected_clip.is_none() {
            return;
        }
        if let Some(length) = self.midi_clip().map(|clip| clip.source_length().raw()) {
            self.fit_midi_interval((0, length), false);
        } else if self.mode == super::EditorMode::Arrangement {
            self.fit_selected_audio_clip();
        }
    }

    /// Fit the clip editor to the current Arrangement time selection when it
    /// covers the clip editor's subject on one of its tracks.
    pub(super) fn fit_clip_editor_to_time_selection(&mut self) -> bool {
        let Some(range) = self.normalized_time_selection() else {
            return false;
        };
        let Some(clip) = self.detail_clip_id() else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let project = &state.project;
        let Some(track) = project.tracks().iter().find(|track| {
            track.clips().iter().any(|c| c.id() == clip)
                || track.midi_clips().iter().any(|c| c.id() == clip)
        }) else {
            return false;
        };
        if !range.tracks().contains(&track.id()) {
            return false;
        }
        let frames = (range.start(), range.end());
        if let Some(placement) = track.midi_clips().iter().find(|c| c.id() == clip) {
            let Some(source) = midi_source_range(project.tempo_map(), placement, frames) else {
                return false;
            };
            return self.fit_midi_interval(source, false);
        }
        let Some(audio) = track.clips().iter().find(|c| c.id() == clip) else {
            return false;
        };
        let whole = super::audio_event_editor::editor_bounds(&audio.audio_event_root());
        let Some(root) = audio_root_range(project.tempo_map(), audio, frames, whole) else {
            return false;
        };
        self.fit_audio_interval(root, false)
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};

    fn beats(quarters: u64) -> BeatTime {
        BeatTime::from_quarters(quarters).unwrap()
    }

    fn editor() -> Editor {
        Editor::new(Project::new(48_000).unwrap(), 8).unwrap()
    }

    /// A 4-beat MIDI placement at beat 1 (24 000 frames at 120 bpm); with
    /// `looping`, its 1-beat source loop repeats four times.
    fn midi(looping: bool) -> (Editor, hex_project::ClipId) {
        let mut editor = editor();
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: "m".into() })
            .unwrap()
        else {
            panic!("track");
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ONE,
                length: beats(4),
            })
            .unwrap()
        else {
            panic!("clip");
        };
        if looping {
            editor
                .execute(Command::SetMidiClipSource {
                    clip,
                    source_length: BeatTime::ONE,
                    source_start: BeatTime::ZERO,
                    loop_start: BeatTime::ZERO,
                    loop_end: BeatTime::ONE,
                    looping: true,
                    length: beats(4),
                })
                .unwrap();
        }
        (editor, clip)
    }

    #[test]
    fn midi_selection_maps_through_tempo_into_source_beats_and_clamps_to_the_clip() {
        let (editor, clip) = midi(false);
        let project = editor.project();
        let placement = project.midi_clip(clip).unwrap();
        let tempo = project.tempo_map();
        let frame = |beat: BeatTime| tempo.beat_to_frame(beat).unwrap();
        let half = BeatTime::ONE.raw() / 2;
        // Beats 2..3 on the timeline are source beats 1..2 of a clip at beat 1.
        assert_eq!(
            midi_source_range(tempo, placement, (frame(beats(2)), frame(beats(3)))),
            Some((BeatTime::ONE.raw(), beats(2).raw()))
        );
        // A range starting before the clip clamps to its start.
        assert_eq!(
            midi_source_range(
                tempo,
                placement,
                (0, frame(BeatTime::from_raw(BeatTime::ONE.raw() + half)))
            ),
            Some((0, half))
        );
        assert_eq!(
            midi_source_range(tempo, placement, (0, frame(BeatTime::ONE))),
            None
        );
    }

    #[test]
    fn looped_midi_selection_folds_into_the_loop_or_fits_the_whole_cycle() {
        let (editor, clip) = midi(true);
        let project = editor.project();
        let placement = project.midi_clip(clip).unwrap();
        let tempo = project.tempo_map();
        let frame = |raw: u64| tempo.beat_to_frame(BeatTime::from_raw(raw)).unwrap();
        let one = BeatTime::ONE.raw();
        let quarter = one / 4;
        // The third repeat's middle half folds onto source 0.25..0.75.
        assert_eq!(
            midi_source_range(
                tempo,
                placement,
                (frame(3 * one + quarter), frame(4 * one - quarter))
            ),
            Some((quarter, one - quarter))
        );
        // Crossing a repeat boundary, or covering a whole cycle, fits the loop.
        let whole = Some((0, one));
        assert_eq!(
            midi_source_range(
                tempo,
                placement,
                (frame(2 * one - quarter), frame(2 * one + quarter))
            ),
            whole
        );
        assert_eq!(
            midi_source_range(tempo, placement, (frame(2 * one), frame(4 * one + quarter))),
            whole
        );
    }

    #[test]
    fn audio_selection_maps_to_root_positions_and_follows_a_looped_placement() {
        let mut editor = editor();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "a".into(),
                path: "missing.wav".into(),
                sample_rate: 48_000,
                frames: 96_000,
            })
            .unwrap()
        else {
            panic!("import");
        };
        let audio = |editor: &Editor| editor.project().clip(clip).unwrap().clone();
        let whole = (0, 96_000);
        let tempo = editor.project().tempo_map().clone();
        assert_eq!(
            audio_root_range(&tempo, &audio(&editor), (24_000, 48_000), whole),
            Some((24_000, 48_000))
        );
        assert_eq!(
            audio_root_range(&tempo, &audio(&editor), (90_000, 200_000), whole),
            Some((90_000, 96_000)),
            "the range clamps to the placement"
        );
        editor
            .execute(Command::SetClipLooping {
                clip,
                looping: true,
            })
            .unwrap();
        editor
            .execute(Command::TrimClipEndBy {
                clip,
                frames: 96_000,
            })
            .unwrap();
        let looped = audio(&editor);
        assert_eq!(looped.duration(), 192_000);
        let projection = hex_project::AudioViewProjection::new(&looped, &tempo).unwrap();
        let root = |frame| projection.root_position_for_frame(frame).unwrap().unwrap();
        let bounds = super::super::audio_event_editor::editor_bounds(&looped.audio_event_root());
        assert!(
            bounds.0 <= root(90_000) && root(119_999) < bounds.1,
            "the editor's fit clamp keeps unfolded repeat positions"
        );
        // The looped editor root spans the placement; a selection in the second
        // repeat lands where the playback line is drawn for those frames.
        assert_eq!(
            audio_root_range(&tempo, &looped, (100_000, 120_000), whole),
            Some((root(100_000), root(119_999) + 1))
        );
        assert_eq!(
            audio_root_range(&tempo, &looped, (90_000, 100_000), whole),
            Some((root(90_000), root(99_999) + 1)),
            "a range across the repeat boundary stays contiguous"
        );
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod ui_tests {
    use super::super::clip_drag_ui_test::{audio_project, editor_window, midi_project};
    use super::BeatTime;
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
    use std::sync::Arc;

    /// Drag a free (unsnapped) time selection over `fractions` of the lane.
    fn drag(visual: &mut VisualTestContext, track: hex_project::TrackId, fractions: (f32, f32)) {
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", track.value()).into_boxed_str(),
            ))
            .expect("lane");
        let at = |fraction: f32| {
            point(
                lane.origin.x + lane.size.width * fraction,
                lane.origin.y + px(22.0),
            )
        };
        let free = Modifiers {
            platform: true,
            ..Default::default()
        };
        visual.simulate_mouse_down(at(fractions.0), MouseButton::Left, free);
        visual.simulate_mouse_move(at(fractions.1), MouseButton::Left, free);
        visual.simulate_mouse_up(at(fractions.1), MouseButton::Left, free);
        visual.run_until_parked();
    }

    fn click_header(visual: &mut VisualTestContext, clip: hex_project::ClipId) {
        let header = visual
            .debug_bounds(Box::leak(
                format!("clip-name-{}", clip.value()).into_boxed_str(),
            ))
            .expect("clip header");
        visual.simulate_click(header.center(), Modifiers::default());
        visual.run_until_parked();
    }

    #[gpui::test]
    fn dragged_selection_fits_the_piano_roll_to_the_selected_source_beats(cx: &mut TestAppContext) {
        let (project, source, _, clip) = midi_project();
        let (_, preview) =
            hex_engine::compile::project_with_midi_preview(&project, &Default::default())
                .expect("midi preview");
        let (handle, editor) = editor_window(cx, project, Arc::new(preview));
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        // Lane fractions of the 384 000-frame view: beats 2..3 of a clip at beat 1.
        drag(&mut visual, source, (0.125, 0.1875));
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.detail_clip_id(), Some(clip));
            let range = view.normalized_time_selection().expect("selection");
            let project = &view.state.as_ref().unwrap().project;
            let expected = super::midi_source_range(
                project.tempo_map(),
                project.midi_clip(clip).unwrap(),
                (range.start(), range.end()),
            )
            .expect("mapped");
            assert!(expected.0 > 0 && expected.1 > expected.0);
            assert_eq!(
                (view.piano_beat_start, view.piano_beat_span.raw()),
                (i128::from(expected.0), expected.1 - expected.0)
            );
        });
        click_header(&mut visual, clip);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.selected_clip, Some(clip));
            assert_eq!(
                (view.piano_beat_start, view.piano_beat_span.raw()),
                (0, 4 * BeatTime::ONE.raw())
            );
        });
        editor.update(&mut visual, |view, cx| {
            assert!(view.fit_midi_interval((BeatTime::ONE.raw(), 2 * BeatTime::ONE.raw()), false));
            cx.notify();
        });
        visual.run_until_parked();
        click_header(&mut visual, clip);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                (view.piano_beat_start, view.piano_beat_span.raw()),
                (0, 4 * BeatTime::ONE.raw())
            );
        });
        editor.update(&mut visual, |view, cx| {
            view.clip_view_open = false;
            assert!(view.fit_midi_interval((BeatTime::ONE.raw(), 2 * BeatTime::ONE.raw()), false));
            cx.notify();
        });
        visual.run_until_parked();
        click_header(&mut visual, clip);
        editor.read_with(&visual, |view, _| {
            assert!(!view.clip_view_open);
            assert_eq!(
                (view.piano_beat_start, view.piano_beat_span.raw()),
                (i128::from(BeatTime::ONE.raw()), BeatTime::ONE.raw())
            );
        });
    }

    #[gpui::test]
    fn dragged_selection_fits_the_audio_editor_to_the_selected_root_range(cx: &mut TestAppContext) {
        let (project, source, _, clip) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        drag(&mut visual, source, (0.0625, 0.125));
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.detail_clip_id(), Some(clip));
            let range = view.normalized_time_selection().expect("selection");
            let project = &view.state.as_ref().unwrap().project;
            let audio = project.clip(clip).unwrap();
            let root = audio.audio_event_root();
            let whole = super::super::audio_event_editor::editor_bounds(&root);
            let (from, to) = super::audio_root_range(
                project.tempo_map(),
                audio,
                (range.start(), range.end()),
                whole,
            )
            .expect("mapped");
            assert!(from > 0);
            let first = root
                .root_frame_at(whole.0, project.tempo_map())
                .expect("first");
            let viewport = view.event_viewport(&root);
            assert_eq!(
                (viewport.start, viewport.length),
                (
                    root.root_frame_at(from, project.tempo_map()).unwrap() - first,
                    root.root_frame_at(to, project.tempo_map()).unwrap()
                        - root.root_frame_at(from, project.tempo_map()).unwrap()
                )
            );
        });
        click_header(&mut visual, clip);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.selected_clip, Some(clip));
            let root = view
                .state
                .as_ref()
                .unwrap()
                .project
                .clip(clip)
                .unwrap()
                .audio_event_root();
            let viewport = view.event_frame_window(&root);
            assert_eq!((viewport.start, viewport.length), (0, 96_000));
        });
    }

    #[gpui::test]
    fn selecting_audio_in_source_view_fits_its_crop(cx: &mut TestAppContext) {
        use hex_project::{Command, Editor};

        let (project, _, _, clip) = audio_project();
        let mut model = Editor::new(project, 8).unwrap();
        model
            .execute(Command::TrimClip {
                clip,
                start: 12_000,
                duration: 48_000,
                source_offset: 12_000,
                source_length: 48_000,
            })
            .unwrap();
        let (handle, editor) = editor_window(cx, model.project().clone(), Arc::default());
        editor.update(cx, |view, cx| {
            view.audio_event_editor.source_view = true;
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click_header(&mut visual, clip);
        editor.read_with(&visual, |view, _| {
            let viewport = view.audio_source_viewport();
            assert_eq!((viewport.start, viewport.length), (12_000, 48_000));
        });
    }
}
