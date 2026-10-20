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
    /// Map this exact source context to its current authored view. No older
    /// revision is accepted without the private, persistent worker binding.
    pub fn audio_authoring_revision(&self, context: hex_engine::AudioSourceContext) -> Option<u64> {
        let source = self.canonical_audio_source.as_ref()?;
        if source.context() != context {
            return None;
        }
        if context.editor_revision == self.revision {
            return Some(self.revision);
        }
        self.comparison_audio
            .as_ref()
            .filter(|binding| {
                binding.revision == self.revision && Arc::ptr_eq(&binding.source, source)
            })
            .map(|_| self.revision)
    }
}
