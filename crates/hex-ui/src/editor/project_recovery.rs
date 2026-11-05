//! Explicit native recovery/relink dialogs; storage and media admission remain on workers.
//! Missing media in the chosen source is resolved in the `missing_media` panel.
use super::EditorView;
use gpui::{Context, PathPromptOptions, PromptButton, PromptLevel, Window};
use hex_session::service::{ProjectSource, Request};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Copy)]
pub(super) enum OpenKind {
    Relink,
    Untitled,
    SavedEdits,
    PreviousSave,
    /// A copy kept by discard or copy-on-first-edit: a journal or its folder.
    Kept,
}

impl EditorView {
    pub(super) fn project_recovery_dialog(
        &mut self,
        kind: OpenKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.opening_project
            || self.project_open.busy()
            || self.pending_open_path.is_some()
            || self.close_prompt_open
            || self.saving_project
            || self.save_dialog_open
        {
            self.error = Some("finish the current project operation first".into());
            return;
        }
        if !super::device_panel::finalize_controls(self, cx) {
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let confirmation = if state.unsaved
            || self.worker_requests_pending > 0
            || !self.pending_requests.is_empty()
            || self.device_gesture_pending()
            || state.can_undo
            || state.can_redo
            || !state.project.tracks().is_empty()
            || state.project.assets().next().is_some()
        {
            Some(window.prompt(PromptLevel::Warning, "replace the working project?",
                Some("unsaved work will be replaced only after the selected project and its media are validated."),
                &[PromptButton::cancel("cancel"), PromptButton::new("replace")], cx))
        } else {
            None
        };
        let own_slot = self.service.recovery_slot().map(Path::to_path_buf);
        self.opening_project = true;
        self.pending_open_path = None;
        cx.spawn(async move |this, cx| {
            let result: Result<Option<ProjectSource>, String> = async {
                if let Some(confirmation) = confirmation
                    && confirmation.await.ok() != Some(1)
                {
                    return Ok(None);
                }
                let source = if matches!(kind, OpenKind::Untitled) {
                    own_untitled_source(own_slot.as_deref())?
                } else {
                    let response = this
                        .update(cx, |_, cx| {
                            cx.prompt_for_paths(PathPromptOptions {
                                files: true,
                                // Kept copies may be hidden dot-files: their folder is accepted.
                                directories: matches!(kind, OpenKind::Kept),
                                multiple: false,
                                prompt: Some(if matches!(kind, OpenKind::Kept) {
                                    "choose kept session".into()
                                } else {
                                    "choose saved project".into()
                                }),
                            })
                        })
                        .map_err(|e| e.to_string())?;
                    let Some(mut paths) = response
                        .await
                        .map_err(|e| e.to_string())?
                        .map_err(|e| e.to_string())?
                    else {
                        return Ok(None);
                    };
                    let Some(path) = paths.pop() else {
                        return Ok(None);
                    };
                    match kind {
                        OpenKind::Relink => ProjectSource::Saved(path),
                        OpenKind::SavedEdits => ProjectSource::SavedJournal(path),
                        OpenKind::PreviousSave => ProjectSource::PreviousSave(path),
                        OpenKind::Kept => hex_session::service::kept_journal(&path)?,
                        OpenKind::Untitled => return Ok(None),
                    }
                };
                Ok(Some(source))
            }
            .await;
            let _ = this.update(cx, |view, cx| {
                view.opening_project = false;
                match result {
                    // Missing files open the relink panel instead of failing.
                    Ok(Some(source)) => view.open_checking_missing_media(
                        source.clone(),
                        hex_session::DocumentAudioMode::Resident,
                        None,
                        Request::OpenWith {
                            source,
                            relinks: BTreeMap::new(),
                        },
                        cx,
                    ),
                    Ok(None) => view.status = "project recovery cancelled".into(),
                    Err(reason) => view.error = Some(reason.to_lowercase().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl EditorView {
    pub(super) fn restart_recovery_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recovery_prompt_open
            || self.opening_project
            || self.project_open.busy()
            || self.close_prompt_open
            || self.saving_project
            || self.save_dialog_open
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || self.device_gesture_pending()
            || self.device_panel.has_pending_draft(cx)
        {
            self.error =
                Some("finish the current project operation before restarting recovery".into());
            cx.notify();
            return;
        }
        let Some(failure) = self
            .recovery_failure
            .as_ref()
            .filter(|failure| failure.can_restart())
        else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let intent = hex_session::service::RecoveryRestart {
            project_generation: self.project_generation,
            revision: state.revision,
            failure_id: failure.id,
        };
        if failure.project_generation != intent.project_generation {
            return;
        }
        let message = crate::theme::text("start a fresh recovery journal?");
        let detail = crate::theme::text(
            "the existing journal will be preserved. a new checkpoint will be written for the working project. projects and media will not be changed.",
        );
        let confirmation = window.prompt(
            PromptLevel::Warning,
            &message,
            Some(&detail),
            &[
                PromptButton::cancel(crate::theme::text("cancel")),
                PromptButton::new(crate::theme::text("start fresh")),
            ],
            cx,
        );
        self.recovery_prompt_open = true;
        cx.spawn(async move |this, cx| {
            let accepted = confirmation.await.ok() == Some(1);
            let _ = this.update(cx, |view, cx| {
                view.recovery_prompt_open = false;
                if !accepted {
                    view.status = "automatic recovery restart cancelled".into();
                } else if view.project_generation != intent.project_generation
                    || view
                        .state
                        .as_ref()
                        .is_none_or(|state| state.revision != intent.revision)
                    || view
                        .recovery_failure
                        .as_ref()
                        .is_none_or(|failure| failure.id != intent.failure_id)
                {
                    view.error = Some(
                        "recovery restart cancelled because the working project changed".into(),
                    );
                } else if view.enqueue(Request::RestartRecovery(intent)) {
                    view.status = "starting fresh automatic recovery".into();
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

/// `file > recover latest untitled project…` opens only this window's own
/// claimed untitled slot, never another live instance's (Pulse #4575).
/// Leftovers of ended instances are offered at launch instead.
fn own_untitled_source(slot: Option<&Path>) -> Result<ProjectSource, String> {
    slot.map(|slot| ProjectSource::Journal(slot.to_path_buf()))
        .ok_or_else(|| "this window has no untitled recovery slot".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untitled_recovery_opens_only_the_window_s_own_slot() {
        let own = Path::new("/recovery/untitled-3.hex-journal");
        assert!(matches!(
            own_untitled_source(Some(own)),
            Ok(ProjectSource::Journal(path)) if path == own
        ));
        assert_eq!(
            own_untitled_source(None).err().as_deref(),
            Some("this window has no untitled recovery slot")
        );
    }
}
