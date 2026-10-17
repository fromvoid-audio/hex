//! Whole Arrangement clip membership; time selections keep their separate owner.
use super::EditorView;
use gpui::{Context, Modifiers, Window};
use hex_project::{ClipId, Project, TrackId};
use std::collections::BTreeSet;

#[derive(Clone, Default)]
pub(super) struct ClipSelection {
    members: BTreeSet<ClipId>,
    anchor: Option<(TrackId, ClipId)>,
    scope: Option<(u64, u64)>,
    pub(super) epoch: u64,
}
impl ClipSelection {
    pub(super) fn only(&mut self, track: TrackId, clip: ClipId) {
        self.apply_members(BTreeSet::from([clip]));
        self.anchor = Some((track, clip));
    }
    fn apply_members(&mut self, members: BTreeSet<ClipId>) {
        if self.members != members {
            self.members = members;
            self.epoch = self.epoch.wrapping_add(1);
        }
    }
    fn select(&mut self, project: &Project, track: TrackId, clip: ClipId, modifiers: Modifiers) {
        if modifiers.shift
            && let Some((anchor_track, anchor_clip)) = self.anchor
            && anchor_track == track
        {
            let ordered = ordered_clips(project, track);
            if let Some(a) = ordered.iter().position(|id| *id == anchor_clip)
                && let Some(b) = ordered.iter().position(|id| *id == clip)
            {
                let mut members = if modifiers.platform {
                    self.members.clone()
                } else {
                    BTreeSet::new()
                };
                members.extend(&ordered[a.min(b)..=a.max(b)]);
                self.apply_members(members);
                return;
            }
        }
        if modifiers.platform {
            let mut members = self.members.clone();
            if !members.remove(&clip) {
                members.insert(clip);
            }
            self.apply_members(members);
            self.anchor = Some((track, clip));
        } else {
            self.only(track, clip);
        }
    }
    fn reconcile(&mut self, project: &Project, generation: u64, revision: u64) {
        if self.scope == Some((generation, revision)) {
            return;
        }
        if self.scope.is_some_and(|(old, _)| old != generation) {
            self.apply_members(BTreeSet::new());
            self.anchor = None;
        }
        self.scope = Some((generation, revision));
        if self.members.is_empty() && self.anchor.is_none() {
            return;
        }
        let valid = project
            .tracks()
            .iter()
            .flat_map(|track| {
                track
                    .clips()
                    .iter()
                    .map(|c| c.id())
                    .chain(track.midi_clips().iter().map(|c| c.id()))
            })
            .collect::<BTreeSet<_>>();
        let members = self.members.intersection(&valid).copied().collect();
        self.apply_members(members);
        if self.anchor.is_some_and(|(_, id)| !valid.contains(&id)) {
            self.anchor = None;
        }
    }
}
fn ordered_clips(project: &Project, track: TrackId) -> Vec<ClipId> {
    let Some(track) = project.track(track) else {
        return Vec::new();
    };
    let mut clips = track
        .clips()
        .iter()
        .map(|c| (c.start(), c.id()))
        .chain(track.midi_clips().iter().filter_map(|c| {
            project
                .tempo_map()
                .beat_to_frame(c.start())
                .ok()
                .map(|frame| (frame, c.id()))
        }))
        .collect::<Vec<_>>();
    clips.sort_unstable();
    clips.into_iter().map(|(_, id)| id).collect()
}
impl EditorView {
    pub(super) fn reconcile_clip_selection(&mut self) {
        // A lane/header focus change clears the visible clip selection. A later
        // additive click must not resurrect those now invisible members.
        if self.selected_clip.is_none() {
            self.clip_selection.apply_members(BTreeSet::new());
            self.clip_selection.anchor = None;
        }
        if let Some(state) = &self.state {
            self.clip_selection
                .reconcile(&state.project, self.project_generation, state.revision);
        }
    }
    pub(super) fn clip_is_selected(&self, clip: ClipId) -> bool {
        self.selected_clip == Some(clip)
            || self.selected_clip.is_some_and(|primary| {
                self.clip_selection.members.contains(&primary)
                    && self.clip_selection.members.contains(&clip)
            })
    }
    pub(super) fn selected_clip_ids(&self) -> Vec<ClipId> {
        let Some(primary) = self.selected_clip else {
            return Vec::new();
        };
        if self.clip_selection.members.contains(&primary) {
            self.clip_selection.members.iter().copied().collect()
        } else {
            vec![primary]
        }
    }
    pub(super) fn select_clip_header(
        &mut self,
        track: TrackId,
        clip: ClipId,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reconcile_clip_selection();
        let mut selection = self.clip_selection.clone();
        if !self.set_selected_clip(track, clip, window, cx) {
            return;
        }
        let mut next_focus = None;
        if let Some(state) = &self.state {
            selection.select(&state.project, track, clip, modifiers);
            if !selection.members.contains(&clip) {
                self.selected_clip = None;
                if let Some(primary) = selection.members.iter().next_back().copied() {
                    next_focus = state
                        .project
                        .tracks()
                        .iter()
                        .find(|t| {
                            t.clips().iter().any(|c| c.id() == primary)
                                || t.midi_clips().iter().any(|c| c.id() == primary)
                        })
                        .map(|t| (t.id(), primary));
                }
            }
        }
        if let Some((owner, primary)) = next_focus {
            self.set_selected_clip(owner, primary, window, cx);
        }
        self.clip_selection = selection;
        let count = self.selected_clip_ids().len();
        self.status = format!("{count} clips selected");
        cx.notify();
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{MouseButton, TestAppContext, VisualTestContext};
    use hex_project::{BeatTime, Command, Created, Editor};
    use std::sync::Arc;

    fn fixture() -> (Project, Vec<ClipId>, ClipId) {
        let (project, track, target, imported) = super::super::clip_drag_ui_test::audio_project();
        let asset = project.clip(imported).unwrap().asset();
        let mut editor = Editor::new(project, 32).unwrap();
        editor.execute(Command::RemoveClip(imported)).unwrap();
        let mut clips = Vec::new();
        for index in 0..5 {
            let start = index * 36_000;
            let command = if index % 2 == 0 {
                Command::PlaceClip {
                    track,
                    asset,
                    start,
                    duration: 24_000,
                    source_offset: 0,
                    source_length: 24_000,
                    looping: false,
                }
            } else {
                Command::PlaceMidiClip {
                    track,
                    start: editor.project().tempo_map().frame_to_beat(start).unwrap(),
                    length: BeatTime::ONE,
                }
            };
            let Created::Clip(id) = editor.execute(command).unwrap() else {
                panic!("clip")
            };
            clips.push(id);
        }
        let Created::Clip(other) = editor
            .execute(Command::PlaceClip {
                track: target,
                asset,
                start: 0,
                duration: 24_000,
                source_offset: 0,
                source_length: 24_000,
                looping: false,
            })
            .unwrap()
        else {
            panic!("other clip")
        };
        (editor.project().clone(), clips, other)
    }
    fn click(visual: &mut VisualTestContext, clip: ClipId, modifiers: Modifiers) {
        let selector = Box::leak(format!("clip-name-{}", clip.value()).into_boxed_str());
        let bounds = visual.debug_bounds(selector).expect("rendered clip header");
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, modifiers);
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, modifiers);
        visual.run_until_parked();
    }
    fn selected(
        editor: &gpui::Entity<EditorView>,
        visual: &mut VisualTestContext,
    ) -> BTreeSet<ClipId> {
        let (members, paints) = editor.read_with(&*visual, |view, _| {
            let members = view
                .selected_clip_ids()
                .into_iter()
                .collect::<BTreeSet<_>>();
            let mut paints = Vec::new();
            for track in view.state.as_ref().unwrap().project.tracks() {
                for (id, flavor, prefix) in track
                    .clips()
                    .iter()
                    .map(|c| (c.id(), c.flavor(), "clip-visual"))
                    .chain(
                        track
                            .midi_clips()
                            .iter()
                            .map(|c| (c.id(), c.flavor(), "midi-clip-visual")),
                    )
                {
                    paints.push((
                        id,
                        prefix,
                        crate::theme::clip_fill(track.color(), flavor, members.contains(&id)),
                    ));
                }
            }
            (members, paints)
        });
        for (id, prefix, fill) in paints {
            let selector: &'static str =
                Box::leak(format!("{prefix}-{}", id.value()).into_boxed_str());
            assert_eq!(
                visual.rendered_background(selector),
                Some(gpui::rgb(fill).into()),
                "actual audio/MIDI selected paint for {id:?}"
            );
        }
        members
    }
    #[gpui::test]
    fn shift_header_click_selects_five_mixed_clips_and_keeps_reverse_repeated_anchor(
        cx: &mut TestAppContext,
    ) {
        let (project, clips, _) = fixture();
        let original = project.clone();
        let (handle, editor) =
            super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        click(&mut visual, clips[0], Modifiers::default());
        click(&mut visual, clips[4], shift);
        assert_eq!(
            selected(&editor, &mut visual),
            clips.iter().copied().collect()
        );
        click(&mut visual, clips[2], shift);
        assert_eq!(
            selected(&editor, &mut visual),
            clips[..=2].iter().copied().collect()
        );
        click(&mut visual, clips[4], Modifiers::default());
        click(&mut visual, clips[0], shift);
        assert_eq!(
            selected(&editor, &mut visual),
            clips.iter().copied().collect()
        );
        click(&mut visual, clips[2], shift);
        assert_eq!(
            selected(&editor, &mut visual),
            clips[2..].iter().copied().collect()
        );
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original);
            assert!(view.last_enqueued_edit.is_none());
            assert!(view.active_clip_drag.is_none() && view.time_selection_gesture.is_none());
        });
    }
    #[gpui::test]
    fn additive_clip_clicks_toggle_members_and_shift_on_other_track_starts_new_anchor(
        cx: &mut TestAppContext,
    ) {
        let (project, clips, other) = fixture();
        let target = project.tracks()[1].id();
        let (handle, editor) =
            super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let command = Modifiers {
            platform: true,
            ..Modifiers::default()
        };
        click(&mut visual, clips[1], Modifiers::default());
        click(&mut visual, clips[4], command);
        assert_eq!(
            selected(&editor, &mut visual),
            BTreeSet::from([clips[1], clips[4]])
        );
        click(
            &mut visual,
            clips[2],
            Modifiers {
                shift: true,
                ..command
            },
        );
        assert_eq!(
            selected(&editor, &mut visual),
            clips[1..].iter().copied().collect()
        );
        click(&mut visual, clips[2], command);
        assert_eq!(
            selected(&editor, &mut visual),
            BTreeSet::from([clips[1], clips[3], clips[4]])
        );
        click(
            &mut visual,
            other,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(selected(&editor, &mut visual), BTreeSet::from([other]));
        click(&mut visual, other, command);
        assert!(selected(&editor, &mut visual).is_empty());
        click(&mut visual, clips[0], Modifiers::default());
        click(
            &mut visual,
            clips[4],
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", target.value()).into_boxed_str(),
            ))
            .unwrap();
        let blank = gpui::point(
            lane.right() - gpui::px(12.0),
            lane.bottom() - gpui::px(10.0),
        );
        visual.simulate_mouse_down(blank, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(blank, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(selected(&editor, &mut visual).is_empty());
        click(&mut visual, other, command);
        assert_eq!(selected(&editor, &mut visual), BTreeSet::from([other]));
    }
    #[gpui::test]
    fn same_primary_membership_change_repaints_retained_audio_and_midi(cx: &mut TestAppContext) {
        let (project, clips, _) = fixture();
        let (handle, editor) =
            super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click(&mut visual, clips[0], Modifiers::default());
        click(
            &mut visual,
            clips[4],
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(
            selected(&editor, &mut visual),
            clips.iter().copied().collect()
        );
        let primary = editor.read_with(&visual, |view, _| view.selected_clip);
        click(
            &mut visual,
            clips[2],
            Modifiers {
                platform: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(
            editor.read_with(&visual, |view, _| view.selected_clip),
            primary
        );
        assert_eq!(
            selected(&editor, &mut visual),
            BTreeSet::from([clips[0], clips[1], clips[3], clips[4]])
        );
    }
}
