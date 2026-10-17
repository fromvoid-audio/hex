//! Explicit native recovery/relink dialogs; storage and media admission remain on workers.
use super::EditorView;
use gpui::{Context, PathPromptOptions, PromptButton, PromptLevel, Window};
use hex_session::service::{ProjectSource, Request};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(super) enum OpenKind {
    Relink,
    Untitled,
    SavedEdits,
    PreviousSave,
}

impl EditorView {
    pub(super) fn project_recovery_dialog(
        &mut self,
        kind: OpenKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.opening_project
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
        self.opening_project = true;
        self.pending_open_path = None;
        cx.spawn(async move |this, cx| {
            let result: Result<Option<Request>, String> = async {
                if let Some(confirmation) = confirmation {
                    if confirmation.await.ok() != Some(1) {
                        return Ok(None);
                    }
                }
                let source = if matches!(kind, OpenKind::Untitled) {
                    ProjectSource::Journal(hex_session::service::untitled_recovery_path()?)
                } else {
                    let response = this
                        .update(cx, |_, cx| {
                            cx.prompt_for_paths(PathPromptOptions {
                                files: true,
                                directories: false,
                                multiple: false,
                                prompt: Some("choose saved project".into()),
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
                        OpenKind::Untitled => return Ok(None),
                    }
                };
                let inspect_source = source.clone();
                let missing = cx
                    .background_executor()
                    .spawn(async move { hex_session::service::missing_assets(&inspect_source) })
                    .await?;
                let mut relinks = BTreeMap::new();
                for (asset, original) in missing {
                    let response = this
                        .update(cx, |_, cx| {
                            cx.prompt_for_paths(PathPromptOptions {
                                files: true,
                                directories: false,
                                multiple: false,
                                prompt: Some(
                                    format!("locate missing media · {}", original.display()).into(),
                                ),
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
                    relinks.insert(asset, path);
                }
                Ok(Some(Request::OpenWith { source, relinks }))
            }
            .await;
            let _ = this.update(cx, |view, cx| {
                view.opening_project = false;
                match result {
                    Ok(Some(request)) => view.begin_open_request(request),
                    Ok(None) => view.status = "project recovery cancelled".into(),
                    Err(reason) => view.error = Some(reason.to_lowercase().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
