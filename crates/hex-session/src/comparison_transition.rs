//! Existing Editor transactions with a private comparison-only reuse outcome.
use super::*;

pub(crate) struct ComparisonTransition<T> {
    pub(crate) prepared: Arc<hex_engine::PreparedRevision>,
    pub(crate) completed: Option<T>,
    pub(crate) metadata_only: bool,
}

/// Reuse only the actual previous owner across a proved same-audio comparison edge. Audible
/// transitions delegate to the caller's reserved prepare/publication hook before history commits.
pub(crate) fn prepare_comparison<T>(
    source: &Project,
    target: &Project,
    previous: Option<&Arc<hex_engine::PreparedRevision>>,
    prepare: impl FnOnce() -> Result<(Arc<hex_engine::PreparedRevision>, T), SessionError>,
) -> Result<ComparisonTransition<T>, SessionError> {
    if let Some(previous) = previous
        && (source == target || device_comparison::metadata_pair(source, target)?)
    {
        return Ok(ComparisonTransition {
            prepared: Arc::clone(previous),
            completed: None,
            metadata_only: true,
        });
    }
    let (prepared, completed) = prepare()?;
    Ok(ComparisonTransition {
        prepared,
        completed: Some(completed),
        metadata_only: false,
    })
}

impl Session {
    pub(crate) fn comparison_command(
        &mut self,
        command: Command,
        previous: Option<&Arc<hex_engine::PreparedRevision>>,
    ) -> Result<(Created, ComparisonTransition<()>), SessionError> {
        // The worker uses this entry only after typed owner/slot/barrier checks.
        if !matches!(command, Command::CompareDevice { .. }) {
            return Err(SessionError::Invalid("comparison command required"));
        }
        // Split the existing editor out only as a borrow; never replace its history.
        let media = self.media_bank();
        let table = self.sine_table();
        self.editor
            .execute_prepared_transition(command, |source, target| {
                prepare_comparison(source, target, previous, || {
                    let prepared = prepare_with_waveforms(
                        target,
                        &media,
                        table,
                        previous.map(|old| old.session().as_ref()),
                        &self.audio_transforms,
                        &self.audio_clip_waveforms,
                    )
                    .and_then(|prepared| {
                        tuning_prepared_transition(
                            prepared,
                            source,
                            target,
                            previous.map(AsRef::as_ref),
                        )
                    })
                    .map(Arc::new)?;
                    Ok((prepared, ()))
                })
            })
    }

    pub(crate) fn comparison_history<T>(
        &mut self,
        redo: bool,
        previous: Option<&Arc<hex_engine::PreparedRevision>>,
        finish: impl FnOnce(
            &Project,
            &Project,
            &Arc<hex_engine::PreparedRevision>,
        ) -> Result<T, SessionError>,
    ) -> Result<Option<ComparisonTransition<T>>, SessionError> {
        let media = self.media_bank();
        let table = self.sine_table();
        let prepare = |source: &Project, target: &Project| {
            if let Some(previous) = previous
                && device_comparison::metadata_pair(source, target)?
            {
                return Ok(ComparisonTransition {
                    prepared: Arc::clone(previous),
                    completed: None,
                    metadata_only: true,
                });
            }
            let prepared = prepare_with_waveforms(
                target,
                &media,
                table,
                previous.map(|old| old.session().as_ref()),
                &self.audio_transforms,
                &self.audio_clip_waveforms,
            )
            .and_then(|prepared| {
                tuning_prepared_transition(prepared, source, target, previous.map(AsRef::as_ref))
            })
            .map(Arc::new)?;
            let completed = finish(source, target, &prepared)?;
            Ok(ComparisonTransition {
                prepared,
                completed: Some(completed),
                metadata_only: false,
            })
        };
        if redo {
            self.editor.redo_prepared_transition(prepare)
        } else {
            self.editor.undo_prepared_transition(prepare)
        }
    }
}
