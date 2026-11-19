//! Coherent nonowning observation of an actual eligible installation ACK.
//! Each instance has one callback publisher. Repeating the same complete source
//! must not create a false-absence window for bounded control-side readers.
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
        self.publish_between(source, || {});
    }
    fn publish_between(&self, source: Option<AppliedAudioSource>, observe: impl FnOnce()) {
        let before = self.sequence.load(SeqCst);
        // The sole publisher can compare its stable fields without opening a
        // write window. Real changes still invalidate readers before replacing
        // any field; contention is never accepted as an installed source.
        if before & 1 == 0
            && self.root.load(SeqCst) == source.map_or(0, |s| s.output_identity)
            && self.generation.load(SeqCst) == source.map_or(0, |s| s.context.project_generation)
            && self.editor.load(SeqCst) == source.map_or(0, |s| s.context.editor_revision)
            && self.audio.load(SeqCst) == source.map_or(0, |s| s.audio_revision)
        {
            return;
        }
        self.sequence.store(before.wrapping_add(1), SeqCst);
        observe();
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
    pub(super) fn read(&self) -> Option<AppliedAudioSource> {
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
        let source = self.recording_source.read()?;
        (self.applied_revision.load(Acquire) == source.audio_revision).then_some(source)
    }
    /// Control-side request witness for a stopped ordinary input monitor.
    /// The callback still revalidates the exact source, epoch, and runtime.
    pub fn paused_input_source(&self) -> Option<(AppliedAudioSource, u64)> {
        let interval = self.paused_input_interval()?;
        (!self.publication_in_flight()).then_some(interval)
    }
    /// The installed stopped interval even while its successor is preparing.
    /// It can keep existing input custody alive but cannot admit a new monitor.
    pub fn paused_input_interval(&self) -> Option<(AppliedAudioSource, u64)> {
        let epoch = self.transport_epoch.load(Acquire);
        let command = self.admitted_ticket.load(Acquire);
        let source = self.recording_source()?;
        (!self.playing.load(Acquire)
            && !self.closing.load(Acquire)
            && !self.device_fault.load(Acquire)
            && !self.render_fault.load(Acquire)
            && self.pending_stop_marker.load(Acquire) == 0
            && self.consumed_command.load(Acquire) == command
            && self.transport_epoch.load(Acquire) == epoch
            && self.admitted_ticket.load(Acquire) == command
            && self.recording_source() == Some(source))
        .then_some((source, epoch))
    }
    /// A settled paused cursor with retained history eligible for note-monitor
    /// preparation. The publication worker still validates the exact revision,
    /// installed endpoints and runtime scope before rendering.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn paused_note_source(&self) -> Option<(AppliedAudioSource, u64)> {
        let scope = self.paused_input_source()?;
        ((self.position.load(Relaxed) != 0 || self.performance_frame.load(Relaxed) != 0)
            && self.paused_input_source() == Some(scope))
        .then_some(scope)
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
    fn unchanged_source_stays_observable_at_callback_republication_frontier() {
        let status = Status::default();
        let source = AppliedAudioSource {
            context: hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: 7,
            },
            audio_revision: 3,
            output_identity: 9,
        };
        status
            .applied_revision
            .store(source.audio_revision, Release);
        status
            .ordinary_native_revision
            .store(source.audio_revision, Release);
        status.recording_source.publish(Some(source));
        let seen = std::cell::Cell::new(Some(source));
        status
            .recording_source
            .publish_between(Some(source), || seen.set(status.native_effect_source()));
        assert_eq!(
            seen.get(),
            Some(source),
            "an unchanged callback publication must not cancel a live source reader"
        );
        assert_eq!(status.native_effect_source(), Some(source));
    }

    #[test]
    fn real_source_change_and_removal_revoke_reader_before_data_replacement() {
        let source = AppliedSourceStatus::default();
        let first = AppliedAudioSource {
            context: hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: 7,
            },
            audio_revision: 3,
            output_identity: 9,
        };
        let next = AppliedAudioSource {
            audio_revision: 4,
            ..first
        };
        source.publish(Some(first));
        let observed = std::cell::Cell::new(false);
        source.publish_between(Some(next), || {
            observed.set(true);
            assert_eq!(source.read(), None);
        });
        assert!(
            observed.get(),
            "sensitivity: the changed-source write frontier must be exercised"
        );
        assert_eq!(source.read(), Some(next));
        source.publish_between(None, || assert_eq!(source.read(), None));
        assert_eq!(source.read(), None);
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
