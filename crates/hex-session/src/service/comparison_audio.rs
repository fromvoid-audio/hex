//! Sealed authoring correlation. This owns no runtime or publication authority.
use super::*;

#[derive(Clone)]
pub struct ComparisonAudioBinding {
    pub(super) source: Arc<CanonicalAudioSourceSnapshot>,
    previous_revision: u64,
    revision: u64,
}
impl ComparisonAudioBinding {
    pub(super) fn advance(
        previous: &ViewState,
        generation: u64,
        revision: u64,
        session: &Session,
    ) -> Option<Self> {
        let source = previous.canonical_audio_source.as_ref()?;
        if !source.matches_session_sources(session)
            || source.context().project_generation != generation
            || previous.revision.checked_add(1) != Some(revision)
            || previous.audio_authoring_revision(source.context()) != Some(previous.revision)
        {
            return None;
        }
        Some(Self {
            source: Arc::clone(source),
            previous_revision: previous.revision,
            revision,
        })
    }
    /// Ordered worker authoring transition only; not evidence of audio admission.
    pub fn follows(&self, previous: &ViewState, current: &ViewState, generation: u64) -> bool {
        self.previous_revision == previous.revision
            && self.revision == current.revision
            && self.source.context().project_generation == generation
            && previous
                .canonical_audio_source
                .as_ref()
                .is_some_and(|source| Arc::ptr_eq(source, &self.source))
            && current
                .canonical_audio_source
                .as_ref()
                .is_some_and(|source| Arc::ptr_eq(source, &self.source))
            && previous.audio_authoring_revision(self.source.context()) == Some(previous.revision)
            && current.audio_authoring_revision(self.source.context()) == Some(current.revision)
    }
}
impl ViewState {
    /// Map either the exact preparation context or the exact current authored context to this
    /// view. Older preparation aliases require the sealed binding; intermediate revisions and
    /// foreign generations never match. This is authoring correlation, not audio admission.
    pub fn audio_authoring_revision(&self, context: hex_engine::AudioSourceContext) -> Option<u64> {
        let source = self.canonical_audio_source.as_ref()?;
        let prepared = source.context();
        if context.project_generation != prepared.project_generation {
            return None;
        }
        let bound = prepared.editor_revision == self.revision
            || self.comparison_audio.as_ref().is_some_and(|binding| {
                binding.revision == self.revision && Arc::ptr_eq(&binding.source, source)
            });
        (bound && (context == prepared || context.editor_revision == self.revision))
            .then_some(self.revision)
    }
}
