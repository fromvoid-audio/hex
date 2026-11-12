//! Bounded whole-clip duplication, owned through its exact terminal.
//!
//! Repeated intentions preserve normal/alias order without retaining media.
//! Selection changes cancel future intentions, never admitted request custody.
use super::{
    EditorMode, EditorView,
    pending_edit::{Disposition, Receipt},
};
use hex_project::{ClipId, Created, SceneId, SessionCellContent, TrackId};
use hex_session::service::{DuplicateViewObservation, Event, Request, ViewState};
use std::{collections::VecDeque, sync::Arc};

const MAX_QUEUED_REPEATS: u8 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DuplicateKind {
    Independent,
    Alias,
}

#[derive(Default)]
pub(super) struct DuplicateIntent {
    pending: Option<Pending>,
    queued: VecDeque<DuplicateKind>,
}

#[derive(Clone)]
enum Target {
    Arrangement {
        clips: Vec<ClipId>,
        primary: Option<ClipId>,
    },
    Console {
        track: TrackId,
        scene: SceneId,
        clip: ClipId,
    },
}
struct Pending {
    generation: u64,
    revision: u64,
    target: Target,
    kind: DuplicateKind,
    selection_epoch: u64,
    active: bool,
    receipt: Receipt,
    display: Option<Arc<ViewState>>,
}

pub(super) struct Completion {
    pending: Pending,
    created: Option<Created>,
    committed_revision: bool,
    clean: bool,
    queued: VecDeque<DuplicateKind>,
}

impl DuplicateIntent {
    fn queue_repeat(&mut self, kind: DuplicateKind) -> Result<(), &'static str> {
        if !self.pending.as_ref().is_some_and(|p| p.active) {
            return Err("duplicate is still completing");
        }
        if self.queued.len() >= usize::from(MAX_QUEUED_REPEATS) {
            return Err("duplicate queue full · maximum 32 queued repeats");
        }
        self.queued.push_back(kind);
        Ok(())
    }
    pub(super) fn cancel_chain(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.active = false;
        }
        self.queued.clear();
    }
    pub(super) fn reject(&mut self) -> bool {
        self.queued.clear();
        self.pending.take().is_some()
    }
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

impl EditorView {
    fn duplicate_target(&self) -> Option<Target> {
        if self.mode == EditorMode::Session {
            let (track, scene) = self.selected_cell?;
            let clip = match self.state.as_ref()?.project.session_cell(track, scene)? {
                SessionCellContent::Midi(c) => c.id(),
                SessionCellContent::Audio(c) => c.id(),
                SessionCellContent::Stop => return None,
            };
            Some(Target::Console { track, scene, clip })
        } else {
            let clips = self.selected_clip_ids();
            (!clips.is_empty()).then_some(Target::Arrangement {
                clips,
                primary: self.selected_clip,
            })
        }
    }
    fn duplicate_selection_current(&self, pending: &Pending) -> bool {
        pending.active
            && pending.generation == self.project_generation
            && self.selection_intent_epoch == Some(pending.selection_epoch)
            && !self.opening_project
            && self.recovery.generation().is_none()
            && match (&pending.target, self.duplicate_target()) {
                (
                    Target::Arrangement { clips, primary },
                    Some(Target::Arrangement {
                        clips: current,
                        primary: picked,
                    }),
                ) => clips == &current && *primary == picked,
                (
                    Target::Console { track, scene, clip },
                    Some(Target::Console {
                        track: t,
                        scene: s,
                        clip: c,
                    }),
                ) => (*track, *scene, *clip) == (t, s, c),
                _ => false,
            }
    }
    pub(super) fn duplicate_selected(&mut self) {
        self.duplicate_selected_with_kind(DuplicateKind::Independent);
    }
    pub(super) fn duplicate_selected_with_kind(&mut self, kind: DuplicateKind) {
        if self.close_prompt_open || self.automation_close_preflight.frozen() {
            self.error = Some("finish the close action before editing".into());
            return;
        }
        if let Some(pending) = &self.duplicate_intent.pending {
            if !self.duplicate_selection_current(pending) {
                self.duplicate_intent.cancel_chain();
            }
            if let Err(reason) = self.duplicate_intent.queue_repeat(kind) {
                self.error = Some(reason.into());
            }
            return;
        }
        let Some(target) = self.duplicate_target() else {
            return;
        };
        if self.edit_pending() {
            self.error = Some("applying edit · wait before duplicating clips".into());
            return;
        }
        self.start_duplicate(target, kind, VecDeque::new(), false);
    }
    #[cfg(all(test, feature = "ui-test-support"))]
    fn start_clip_duplicate(&mut self, clips: Vec<ClipId>, repeats: u8, continuation: bool) {
        self.start_duplicate(
            Target::Arrangement {
                clips,
                primary: self.selected_clip,
            },
            DuplicateKind::Independent,
            std::iter::repeat_n(DuplicateKind::Independent, usize::from(repeats)).collect(),
            continuation,
        );
    }
    fn start_duplicate(
        &mut self,
        target: Target,
        kind: DuplicateKind,
        queued: VecDeque<DuplicateKind>,
        continuation: bool,
    ) {
        let Some(state) = &self.state else {
            return;
        };
        let Some(selection_epoch) = self.selection_intent_epoch else {
            self.error = Some(
                "selection intent exhausted · reopen the editor before duplicating clips".into(),
            );
            return;
        };
        let revision = state.revision;
        let request = match &target {
            Target::Arrangement { clips, .. } if kind == DuplicateKind::Alias => {
                Request::DuplicateAliasClips {
                    project_generation: self.project_generation,
                    revision,
                    clips: clips.clone(),
                }
            }
            Target::Arrangement { clips, .. } => Request::DuplicateClips {
                project_generation: self.project_generation,
                revision,
                clips: clips.clone(),
            },
            Target::Console { track, scene, .. } => Request::DuplicateSessionCell {
                project_generation: self.project_generation,
                revision,
                track: *track,
                scene: *scene,
                alias: kind == DuplicateKind::Alias,
            },
        };
        self.duplicate_intent.pending = Some(Pending {
            generation: self.project_generation,
            revision,
            target,
            kind,
            selection_epoch,
            active: true,
            receipt: Receipt::new(self.project_generation),
            display: None,
        });
        self.duplicate_intent.queued = queued;
        let accepted = if continuation {
            self.enqueue_duplicate_continuation(request)
        } else {
            self.enqueue(request)
        };
        if !accepted {
            self.duplicate_intent.reject();
        }
    }
    pub(super) fn pending_duplicate_matches(&self, request: &Request) -> bool {
        let Some(pending) = &self.duplicate_intent.pending else {
            return false;
        };
        if !pending.receipt.is_queued() {
            return false;
        }
        match (request, &pending.target) {
            (
                Request::DuplicateClips {
                    project_generation,
                    revision,
                    clips,
                },
                Target::Arrangement {
                    clips: expected, ..
                },
            ) => {
                pending.kind == DuplicateKind::Independent
                    && (*project_generation, *revision) == (pending.generation, pending.revision)
                    && clips == expected
            }
            (
                Request::DuplicateAliasClips {
                    project_generation,
                    revision,
                    clips,
                },
                Target::Arrangement {
                    clips: expected, ..
                },
            ) => {
                pending.kind == DuplicateKind::Alias
                    && (*project_generation, *revision) == (pending.generation, pending.revision)
                    && clips == expected
            }
            (
                Request::DuplicateSessionCell {
                    project_generation,
                    revision,
                    track,
                    scene,
                    alias,
                },
                Target::Console {
                    track: t, scene: s, ..
                },
            ) => {
                (*project_generation, *revision, *track, *scene, *alias)
                    == (
                        pending.generation,
                        pending.revision,
                        *t,
                        *s,
                        pending.kind == DuplicateKind::Alias,
                    )
            }
            _ => false,
        }
    }
    pub(super) fn retire_abandoned_duplicate(&mut self) -> bool {
        let queued = self
            .duplicate_intent
            .pending
            .as_ref()
            .is_some_and(|p| p.receipt.is_queued());
        if queued
            && !self
                .pending_requests
                .iter()
                .chain(self.device_gesture_barriers.iter())
                .any(|request| self.pending_duplicate_matches(request))
        {
            return self.duplicate_intent.reject();
        }
        false
    }
    pub(super) fn duplicate_submitted(&mut self) {
        if let Some(pending) = &mut self.duplicate_intent.pending {
            pending.receipt.submitted(self.worker_requests_pending);
        }
    }
    /// Retain the worker's exact display for this admitted Arrangement copy.
    /// Selection and edit admission still use the ordered terminal.
    pub(super) fn observe_duplicate_view(&mut self, observation: DuplicateViewObservation) -> bool {
        let Some(pending) = self.duplicate_intent.pending.as_mut() else {
            return false;
        };
        if pending.receipt.is_queued()
            || !matches!(pending.target, Target::Arrangement { .. })
            || observation.project_generation != pending.generation
            || observation.previous_revision != pending.revision
        {
            return false;
        }
        let Some(state) = observation.state.upgrade() else {
            return false;
        };
        if pending.revision.checked_add(1) != Some(state.revision) {
            return false;
        }
        pending.display = Some(state);
        self.arrangement_duplicate_view().is_some()
    }

    pub(super) fn arrangement_duplicate_view(&self) -> Option<&Arc<ViewState>> {
        let pending = self.duplicate_intent.pending.as_ref()?;
        let current = self.state.as_ref()?;
        if self.mode != EditorMode::Arrangement
            || self.opening_project
            || self.project_generation != pending.generation
            || current.revision != pending.revision
        {
            return None;
        }
        pending.display.as_ref()
    }

    pub(super) fn take_duplicate_completion(&mut self, event: &mut Event) -> Option<Completion> {
        let disposition = self.duplicate_intent.pending.as_mut()?.receipt.observe(
            self.project_generation,
            event.project_replaced,
            event.request_complete,
        );
        match disposition {
            Disposition::Waiting => None,
            Disposition::Invalidated => {
                self.duplicate_intent.reject();
                None
            }
            Disposition::Terminal => {
                let pending = self.duplicate_intent.pending.take()?;
                let queued = std::mem::take(&mut self.duplicate_intent.queued);
                let committed_revision =
                    pending.revision.checked_add(1) == Some(event.state.revision);
                Some(Completion {
                    pending,
                    queued,
                    committed_revision,
                    clean: event.error.is_none(),
                    created: event.created.take(),
                })
            }
        }
    }
    pub(super) fn complete_clip_duplicate(&mut self, mut completion: Completion) {
        if !completion.committed_revision || !self.duplicate_selection_current(&completion.pending)
        {
            return;
        }
        let selected = match completion.pending.target {
            Target::Arrangement { clips, .. } => {
                let range = match completion.created {
                    Some(Created::Clips { first, last }) => Some((first, last)),
                    Some(Created::Clip(id)) if clips.len() == 1 => Some((id, id)),
                    _ => None,
                };
                range
                    .is_some_and(|(first, last)| self.select_copied_group(first, last, clips.len()))
            }
            Target::Console { track, .. } => {
                let id = match completion.created {
                    Some(Created::SessionClip(id)) => Some(id),
                    _ => None,
                };
                let destination = self.state.as_ref().and_then(|state| {
                    state.project.session_cells().iter().find_map(|cell| {
                        if cell.track() != track {
                            return None;
                        }
                        match cell.content() {
                            SessionCellContent::Midi(c) if Some(c.id()) == id => {
                                Some((cell.scene(), Some(c.id())))
                            }
                            SessionCellContent::Audio(c) if Some(c.id()) == id => {
                                Some((cell.scene(), None))
                            }
                            _ => None,
                        }
                    })
                });
                if let Some((scene, midi)) = destination {
                    if let Some(clip) = midi {
                        self.select_session_midi(track, scene, clip);
                    } else {
                        self.select_session_cell(track, scene);
                    }
                    true
                } else {
                    false
                }
            }
        };
        if !selected {
            self.error
                .get_or_insert_with(|| "copied clips could not be resolved".into());
            return;
        }
        if completion.clean
            && let Some(kind) = completion.queued.pop_front()
            && let Some(target) = self.duplicate_target()
        {
            self.start_duplicate(target, kind, completion.queued, true);
        }
    }
    pub(super) fn make_selected_clips_unique(&mut self) {
        if self.edit_pending() || self.close_prompt_open {
            return;
        }
        let ids = match self.duplicate_target() {
            Some(Target::Arrangement { clips, .. }) => clips,
            Some(Target::Console { clip, .. }) => vec![clip],
            None => return,
        };
        if self
            .state
            .as_ref()
            .is_some_and(|s| ids.iter().any(|id| s.project.clip_alias(*id).is_some()))
        {
            self.enqueue(Request::Edit(hex_project::Command::MakeClipsUnique(ids)));
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "duplicate_ui_tests.rs"]
mod ui_tests;
