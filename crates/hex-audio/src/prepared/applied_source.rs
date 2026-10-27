//! Coherent nonowning observation of an actual eligible installation ACK.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppliedAudioSource {
    pub context: hex_engine::AudioSourceContext,
    pub audio_revision: u64,
    pub output_identity: u64,
}
/// Bounded observation of the callback's installed source. `Pending` is not
/// evidence that a source was removed or replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioSourceObservation {
    Pending,
    Absent,
    Applied(AppliedAudioSource),
}
#[derive(Default)]
pub(super) struct AppliedSourceStatus {
    sequence: AtomicU64,
    root: AtomicU64,
    generation: AtomicU64,
    editor: AtomicU64,
    audio: AtomicU64,
    #[cfg(feature = "allocation-probe")]
    test_contended: AtomicBool,
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
    fn observe(&self) -> AudioSourceObservation {
        #[cfg(feature = "allocation-probe")]
        if self.test_contended.load(Acquire) {
            return AudioSourceObservation::Pending;
        }
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
                return if root != 0 && audio != 0 {
                    AudioSourceObservation::Applied(AppliedAudioSource {
                        context: hex_engine::AudioSourceContext {
                            project_generation: generation,
                            editor_revision: editor,
                        },
                        audio_revision: audio,
                        output_identity: root,
                    })
                } else {
                    AudioSourceObservation::Absent
                };
            }
        }
        AudioSourceObservation::Pending
    }
    pub(super) fn read(&self) -> Option<AppliedAudioSource> {
        match self.observe() {
            AudioSourceObservation::Applied(source) => Some(source),
            AudioSourceObservation::Pending | AudioSourceObservation::Absent => None,
        }
    }
}
impl Status {
    /// Force the bounded recording-source reader to report contention for a
    /// controlled test. This never changes the callback's source or seqlock.
    #[cfg(feature = "allocation-probe")]
    pub fn hold_recording_source_observation_for_test(&self, held: bool) {
        self.recording_source.test_contended.store(held, Release);
    }
    /// Exact installed source for native epoch renewal. Ordinary native effects
    /// reuse the coherent recording-source ACK and its matching native revision;
    /// constructor/sampler `applied_audio_source` keeps its existing meaning.
    /// This observation grants no graph, endpoint or native preparation custody.
    pub fn native_effect_source(&self) -> Option<AppliedAudioSource> {
        self.applied_audio_source().or_else(|| {
            let revision = self.ordinary_native_revision.load(Acquire);
            if revision == 0 {
                return None;
            }
            self.recording_source()
                .filter(|source| source.audio_revision == revision)
        })
    }
    /// Canonical source captured with the actual installed ordinary or eligible
    /// graph. This is correlation, not sink delivery or admission authority.
    pub fn recording_source(&self) -> Option<AppliedAudioSource> {
        match self.recording_source_observation() {
            AudioSourceObservation::Applied(source) => Some(source),
            AudioSourceObservation::Pending | AudioSourceObservation::Absent => None,
        }
    }
    /// Distinguish a stable absence from a contended or not-yet-coherent
    /// callback observation. Neither result grants source custody.
    pub fn recording_source_observation(&self) -> AudioSourceObservation {
        match self.recording_source.observe() {
            AudioSourceObservation::Applied(source)
                if self.applied_revision.load(Acquire) != source.audio_revision =>
            {
                AudioSourceObservation::Pending
            }
            result => result,
        }
    }
    /// Actual active constructor source, independent of delivery of Submitted
    /// events. None includes bounded-reader contention or a legacy install.
    /// This correlation observation grants neither custody nor readiness.
    pub fn applied_audio_source(&self) -> Option<AppliedAudioSource> {
        let source = self.applied_source.read()?;
        (self.applied_revision.load(Acquire) == source.audio_revision).then_some(source)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recording_source_distinguishes_contention_from_stable_absence() {
        let status = Status::default();
        assert_eq!(
            status.recording_source_observation(),
            AudioSourceObservation::Absent
        );
        let source = AppliedAudioSource {
            context: hex_engine::AudioSourceContext {
                project_generation: 2,
                editor_revision: 7,
            },
            audio_revision: 3,
            output_identity: 9,
        };
        status.recording_source.publish(Some(source));
        assert_eq!(
            status.recording_source_observation(),
            AudioSourceObservation::Pending
        );
        status.applied_revision.store(3, Release);
        assert_eq!(
            status.recording_source_observation(),
            AudioSourceObservation::Applied(source)
        );
        status.recording_source.sequence.store(3, SeqCst);
        assert_eq!(
            status.recording_source_observation(),
            AudioSourceObservation::Pending
        );
        status.recording_source.sequence.store(4, SeqCst);
        status.recording_source.publish(None);
        assert_eq!(
            status.recording_source_observation(),
            AudioSourceObservation::Absent
        );
    }
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
