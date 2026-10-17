//! Exact fresh-constructor runtime DATA; never a generic Playback declaration.
use super::*;
use crate::{
    AudioRootStorageLedger, AudioStorageError,
    audio_storage::{Location, Owner},
};
impl Playback {
    pub(crate) fn candidate_constructor_is_fresh(&self) -> bool {
        self.session_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.require_fresh_candidate().is_ok())
    }
    pub(crate) fn visit_fresh_candidate_storage<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let Self {
            metronome,
            project,
            project_track_ids: _,
            prepared_revision,
            session_runtime,
            linear_runtime,
            session_unavailable,
            position: _,
            performance_frame: _,
            performance_clock_exhausted: _,
            playing: _,
        } = self;
        if linear_runtime.is_some() || session_unavailable.is_some() {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        let revision = prepared_revision
            .as_ref()
            .ok_or(AudioStorageError::UnsupportedGraph)?;
        if !Arc::ptr_eq(project, revision.arrangement()) {
            return Err(AudioStorageError::SourceMismatch);
        }
        ledger.existing_shared(
            project,
            Owner::exclusive(project),
            Location("playback-project", 0),
        )?;
        ledger.existing_shared(
            revision,
            Owner::exclusive(revision),
            Location("playback-revision", 0),
        )?;
        let session = session_runtime
            .as_ref()
            .ok_or(AudioStorageError::UnsupportedGraph)?;
        ledger.bytes(
            Owner::exclusive(session),
            Location("session-box", 0),
            std::mem::size_of::<SessionPlayback>(),
        )?;
        session.visit_fresh_candidate_storage(ledger)?;
        let metronome = metronome
            .as_ref()
            .ok_or(AudioStorageError::UnknownStorage)?;
        ledger.bytes(
            Owner::exclusive(metronome),
            Location("metronome-box", 0),
            std::mem::size_of::<Metronome>(),
        )?;
        metronome.visit_candidate_storage(ledger)
    }
}
