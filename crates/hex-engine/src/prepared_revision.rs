//! Exact control-side products for one accepted canonical project revision.
use crate::{
    Invalid, MidiArrangementPreview, PreparedSessionInventory, Project as PlaybackProject,
    compile::{
        PreparedAudioSource, project_with_midi_preview_and_table,
        project_with_prepared_audio_sources_and_midi_preview_and_table,
    },
};
use hex_devices::SineTable;
use hex_project::{AssetId, Project};
use std::{collections::BTreeMap, sync::Arc};

/// Immutable compiled outputs prepared before a canonical revision is committed.
/// Arrangement playback and Session inventory share a stable instrument table;
/// runtime route ownership is maintained by the playback kernel.
pub struct PreparedRevision {
    arrangement: Arc<PlaybackProject>,
    session: Arc<PreparedSessionInventory>,
    midi_preview: Arc<MidiArrangementPreview>,
}

impl std::fmt::Debug for PreparedRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedRevision")
            .field("arrangement_rate", &self.arrangement.sample_rate())
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl PreparedRevision {
    pub fn prepare(
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
    ) -> Result<Self, Invalid> {
        let (arrangement, midi_preview) =
            project_with_midi_preview_and_table(project, media, Some(Arc::clone(&sine_table)))?;
        let session = PreparedSessionInventory::prepare_with_media(
            project,
            media,
            sine_table,
            previous_session,
        )?;
        Ok(Self {
            arrangement: Arc::new(arrangement),
            session: Arc::new(session),
            midi_preview: Arc::new(midi_preview),
        })
    }

    /// Prepare Arrangement playback and the Session inventory from one mixed
    /// resident/streamed bank. Streamed bindings are validated against the
    /// supplied nonzero project-open generation by the shared compiler resolver.
    pub fn prepare_with_prepared_audio_sources(
        project: &Project,
        sources: &BTreeMap<AssetId, PreparedAudioSource>,
        expected_generation: u64,
        sine_table: Arc<SineTable>,
        previous_session: Option<&PreparedSessionInventory>,
    ) -> Result<Self, Invalid> {
        let (arrangement, midi_preview) =
            project_with_prepared_audio_sources_and_midi_preview_and_table(
                project,
                sources,
                expected_generation,
                Some(Arc::clone(&sine_table)),
            )?;
        let session = PreparedSessionInventory::prepare_with_prepared_audio_sources(
            project,
            sources,
            expected_generation,
            sine_table,
            previous_session,
        )?;
        Ok(Self {
            arrangement: Arc::new(arrangement),
            session: Arc::new(session),
            midi_preview: Arc::new(midi_preview),
        })
    }

    pub fn arrangement(&self) -> &Arc<PlaybackProject> {
        &self.arrangement
    }

    pub fn session(&self) -> &Arc<PreparedSessionInventory> {
        &self.session
    }

    pub fn midi_preview(&self) -> &Arc<MidiArrangementPreview> {
        &self.midi_preview
    }

    pub fn contains_streamed_sources(&self) -> bool {
        self.arrangement.contains_streamed_sources() || self.session.contains_streamed_sources()
    }
}
