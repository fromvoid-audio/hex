//! Explicit owned root. Views expose leaf metadata, never a raw graph owner.
use crate::{PreparedRevision, owned_shared::OwnedShared};
#[derive(Debug)]
pub struct OwnedPreparedRevision {
    pub(crate) owner: OwnedShared<PreparedRevision>,
}
impl OwnedPreparedRevision {
    pub(crate) fn new(value: PreparedRevision) -> Self {
        Self {
            owner: OwnedShared::new_owned(value),
        }
    }
    /// Restricted view; no raw graph owner or Deref/AsRef route.
    /// ```compile_fail
    /// fn raw(owner: &hex_engine::OwnedPreparedRevision) {
    ///     let _ = owner.metadata().arrangement();
    /// }
    /// ```
    pub fn metadata(&self) -> OwnedPreparedRevisionView<'_> {
        OwnedPreparedRevisionView {
            prepared: self.owner.as_ref(),
        }
    }
    /// Control-side sharing only; not a callback operation or admission receipt.
    pub fn clone_on_control(&self) -> Self {
        Self {
            owner: self.owner.clone_on_control(),
        }
    }
}
#[derive(Clone, Copy)]
pub struct OwnedPreparedRevisionView<'a> {
    pub(crate) prepared: &'a PreparedRevision,
}
impl<'a> OwnedPreparedRevisionView<'a> {
    pub fn sample_rate(self) -> u32 {
        self.prepared.arrangement().sample_rate()
    }
    pub fn contains_streamed_sources(self) -> bool {
        self.prepared.contains_streamed_sources()
    }
    pub fn arrangement_seek_requires_preroll(self) -> bool {
        self.prepared.arrangement_seek_requires_preroll()
    }
    pub fn streamed_sources(self) -> impl Iterator<Item = &'a crate::StreamedSourceMetadata> {
        self.prepared.streamed_sources()
    }
}

impl OwnedPreparedRevision {
    /// Explicit control compilation. No legacy Arc intake or try_unwrap upgrades.
    pub fn prepare_media_bank(
        project: &hex_project::Project,
        resident: &std::collections::BTreeMap<hex_project::AssetId, crate::Audio>,
        sources: &std::collections::BTreeMap<
            hex_project::AssetId,
            crate::compile::PreparedAudioSource,
        >,
        generation: Option<u64>,
        sine: std::sync::Arc<crate::SineTable>,
        previous: Option<&Self>,
    ) -> Result<Self, crate::Invalid> {
        PreparedRevision::prepare_media_bank(
            project,
            resident,
            sources,
            generation,
            sine,
            previous.map(|owner| owner.owner.session().as_ref()),
        )
        .map(PreparedRevision::into_owned_shared)
    }
}
impl PreparedRevision {
    /// Common owned compile-value path used before either kind of root allocation.
    pub fn prepare_media_bank(
        project: &hex_project::Project,
        resident: &std::collections::BTreeMap<hex_project::AssetId, crate::Audio>,
        sources: &std::collections::BTreeMap<
            hex_project::AssetId,
            crate::compile::PreparedAudioSource,
        >,
        generation: Option<u64>,
        sine: std::sync::Arc<crate::SineTable>,
        previous: Option<&crate::PreparedSessionInventory>,
    ) -> Result<Self, crate::Invalid> {
        match generation {
            Some(generation) => Self::prepare_with_prepared_audio_sources(
                project, sources, generation, sine, previous,
            ),
            None => Self::prepare(project, resident, sine, previous),
        }
    }
}

impl<'a> OwnedPreparedRevisionView<'a> {
    pub fn track_ids(self) -> impl Iterator<Item = hex_project::TrackId> + 'a {
        self.prepared
            .session()
            .tracks()
            .iter()
            .map(|track| track.id)
    }
    pub fn scene_names(self) -> impl Iterator<Item = (hex_project::SceneId, &'a str)> + 'a {
        self.prepared
            .session()
            .scenes()
            .iter()
            .map(|scene| (scene.id, scene.name.as_str()))
    }
    pub fn midi_preview_samples(
        self,
        clip: hex_project::ClipId,
    ) -> Option<&'a [crate::MidiPreviewSample]> {
        self.prepared
            .midi_preview()
            .get(clip)
            .map(|preview| preview.samples.as_slice())
    }
}
