//! Coherent nonowning observation of an actual eligible installation ACK.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppliedAudioSource {
    pub context: hex_engine::AudioSourceContext,
    pub audio_revision: u64,
    pub output_identity: u64,
}
#[derive(Default)]
pub(super) struct AppliedSourceStatus {
    sequence: AtomicU64,
    root: AtomicU64,
    generation: AtomicU64,
    editor: AtomicU64,
    audio: AtomicU64,
}
impl AppliedSourceStatus {
    pub(super) fn publish(&self, source: Option<AppliedAudioSource>) {
        let before = self.sequence.load(SeqCst);
        self.sequence.store(before.wrapping_add(1), SeqCst);
        self.root
            .store(source.map_or(0, |s| s.output_identity), SeqCst);
        self.generation
            .store(source.map_or(0, |s| s.context.project_generation), SeqCst);
        self.editor
            .store(source.map_or(0, |s| s.context.editor_revision), SeqCst);
        self.audio
            .store(source.map_or(0, |s| s.audio_revision), SeqCst);
        self.sequence.store(before.wrapping_add(2), SeqCst);
    }
    fn read(&self) -> Option<AppliedAudioSource> {
        for _ in 0..3 {
            let before = self.sequence.load(SeqCst);
            if before & 1 != 0 {
                continue;
            }
            let root = self.root.load(SeqCst);
            let generation = self.generation.load(SeqCst);
            let editor = self.editor.load(SeqCst);
            let audio = self.audio.load(SeqCst);
            if before == self.sequence.load(SeqCst) {
                return (root != 0 && audio != 0).then_some(AppliedAudioSource {
                    context: hex_engine::AudioSourceContext {
                        project_generation: generation,
                        editor_revision: editor,
                    },
                    audio_revision: audio,
                    output_identity: root,
                });
            }
        }
        None
    }
}
impl Status {
    /// Actual active constructor source, independent of delivery of Submitted
    /// events. None includes bounded-reader contention or a legacy install.
    /// This correlation observation grants neither custody nor readiness.
    pub fn applied_audio_source(&self) -> Option<AppliedAudioSource> {
        let source = self.applied_source.read()?;
        (self.applied_revision.load(Acquire) == source.audio_revision).then_some(source)
    }
}

impl AppliedSourceStatus {
    pub(super) fn audit_scalar_storage(&self) {
        let Self {
            sequence,
            root,
            generation,
            editor,
            audio,
        } = self;
        let _: &AtomicU64 = sequence;
        let _: &AtomicU64 = root;
        let _: &AtomicU64 = generation;
        let _: &AtomicU64 = editor;
        let _: &AtomicU64 = audio;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_unacked_contended_or_legacy_source_is_never_reported_as_applied() {
        let status = Status::default();
        let source = AppliedAudioSource {
            context: hex_engine::AudioSourceContext {
                project_generation: 2,
                editor_revision: 7,
            },
            audio_revision: 3,
            output_identity: 9,
        };
        status.applied_source.publish(Some(source));
        assert_eq!(status.applied_audio_source(), None);
        status.applied_revision.store(3, Release);
        assert_eq!(status.applied_audio_source(), Some(source));
        status.applied_source.sequence.store(3, SeqCst);
        assert_eq!(status.applied_audio_source(), None);
        status.applied_source.sequence.store(4, SeqCst);
        status.applied_source.publish(None);
        assert_eq!(status.applied_audio_source(), None);
    }
}
