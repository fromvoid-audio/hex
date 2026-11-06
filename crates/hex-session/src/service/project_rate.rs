//! Control-side project clock replacement. The existing document media and
//! output admission paths prepare the successor. The saved file handle stays
//! with the caller, and the original Editor keeps every canonical history edge.
use super::{WorkerEvents, resident_audio, sampler_export};
use crate::{DocumentAudioMode, Session};
use hex_engine::AudioSourceContext;
use hex_project::{Editor, Project, SampleRateMode};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectSampleRate {
    FollowInterface,
    Fixed(u32),
}

pub(super) fn new_following_interface(
    mut session: Session,
    history_capacity: usize,
) -> Result<Session, crate::SessionError> {
    let project = session
        .project()
        .clone()
        .with_sample_rate_mode(SampleRateMode::FollowInterface);
    session.editor = Editor::new(project, history_capacity)?;
    Ok(session)
}

pub(super) enum Transition {
    Apply(ProjectSampleRate),
    History { redo: bool },
}

/// Rate changes rebuild their derivatives. A later history step can also
/// restore an asset that was absent when the new derivative bank was built.
pub(super) fn history_needs_replacement(session: &Session, redo: bool) -> bool {
    session
        .editor
        .inspect_history(redo, |source, target| {
            source.sample_rate() != target.sample_rate()
                || target.assets().any(|asset| {
                    let id = asset.id();
                    !target.asset_offline(id)
                        && if asset.original_source().is_some() {
                            !session.native_assets.contains(id)
                        } else {
                            !session.media.contains_key(&id)
                                && !session.verified_sources.contains_key(&id)
                        }
                })
        })
        .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn replace(
    session: &mut Session,
    transition: Transition,
    history_capacity: usize,
    media_base: Option<&Path>,
    context: AudioSourceContext,
    events: &mut WorkerEvents,
    current: Option<&resident_audio::Snapshot>,
    admission: sampler_export::Admission<'_>,
) -> Result<Option<resident_audio::PreparedDocumentReplacement>, String> {
    let selected = match transition {
        Transition::Apply(ProjectSampleRate::FollowInterface) => Some((
            events
                .default_project_sample_rate()
                .ok_or("interface sample rate is unavailable")?,
            SampleRateMode::FollowInterface,
        )),
        Transition::Apply(ProjectSampleRate::Fixed(rate)) => Some((rate, SampleRateMode::Fixed)),
        Transition::History { .. } => None,
    };
    if selected
        == Some((
            session.project().sample_rate(),
            session.project().sample_rate_mode(),
        ))
    {
        return Ok(None);
    }
    if events.native_audio_requested()
        && session.audio_mode() == DocumentAudioMode::VerifiedStreaming
    {
        return Err("sample rate changes require an ordinary project output; reopen the project in ordinary mode".into());
    }
    // Preparation borrows the old Session for admission and source limits.
    // Give it the same current project while the original history owner runs
    // its prepare-before-commit transaction. Only this worker sees the swap.
    let temporary =
        Editor::new(session.project().clone(), history_capacity).map_err(|e| e.to_string())?;
    let mut editor = std::mem::replace(&mut session.editor, temporary);
    let prepare = |target: &Project| -> Result<_, PreparationError> {
        let candidate = resident_audio::prepare_document_media(
            session,
            events,
            current,
            admission,
            |allowance, admission| {
                super::recovery::prepare_project(
                    session,
                    history_capacity,
                    target.clone(),
                    media_base.unwrap_or_else(|| Path::new(".")),
                    session.audio_mode(),
                    session.source_plan_reservation(),
                    session.native_reservation,
                    allowance,
                    admission,
                )
            },
        )?;
        let prepared = resident_audio::prepare_document_replacement(
            &candidate, session, context, events, current, admission,
        )?;
        Ok((candidate, prepared))
    };
    let result = match (transition, selected) {
        (_, Some((rate, mode))) => editor.change_sample_rate_prepared(rate, mode, prepare),
        (Transition::History { redo: true }, _) => editor.redo_prepared(prepare),
        (Transition::History { redo: false }, _) => editor.undo_prepared(prepare),
        (Transition::Apply(_), None) => {
            Err("sample rate choice could not be resolved".to_owned().into())
        }
    };
    session.editor = editor;
    match result {
        Ok(Some((mut candidate, prepared))) => {
            std::mem::swap(&mut candidate.editor, &mut session.editor);
            *session = candidate;
            Ok(Some(prepared))
        }
        Ok(None) => Ok(None),
        Err(error) => Err(error.0),
    }
}

struct PreparationError(String);

impl From<String> for PreparationError {
    fn from(reason: String) -> Self {
        Self(reason)
    }
}

impl From<hex_project::Invalid> for PreparationError {
    fn from(reason: hex_project::Invalid) -> Self {
        Self(reason.to_string())
    }
}
