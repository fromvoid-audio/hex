//! Optional resident mute flow through the ordinary publication owner. A stale
//! proof disarms the ramp without blocking the existing mute transaction.
use super::*;

impl From<hex_engine::playback::ResidentMutePredecessor> for AppliedAudioSource {
    fn from(source: hex_engine::playback::ResidentMutePredecessor) -> Self {
        Self {
            context: source.context,
            output_identity: source.output_identity,
            audio_revision: source.audio_revision,
        }
    }
}

impl PublicationControl {
    pub(super) fn resident_mute_fence(
        &self,
        playback: &Playback,
        policy: PublicationPolicy,
        candidate: Option<hex_engine::AudioSourceContext>,
    ) -> Option<FreshRuntimeFence> {
        let predecessor = AppliedAudioSource::from(playback.resident_mute_predecessor()?);
        let context = candidate?;
        if policy != PublicationPolicy::InheritRuntime
            || predecessor.output_identity != self.candidate_root.root_id()
            || self.status.recording_source() != Some(predecessor)
            || context.project_generation != predecessor.context.project_generation
            || predecessor.context.editor_revision.checked_add(1) != Some(context.editor_revision)
        {
            return None;
        }
        self.capture_running_transport_fence()
            .map(|(_, fence)| fence)
            .filter(|fence| fence.expected_active_revision == predecessor.audio_revision)
    }
}

impl Callback {
    pub(super) fn revalidate_resident_mute(
        status: &Status,
        active: Option<&Publication>,
        candidate: &mut Publication,
    ) {
        let Some(predecessor) = candidate.playback.resident_mute_predecessor() else {
            return;
        };
        let current = candidate.resident_mute_fence.is_some_and(|fence| {
            status.recording_source() == Some(predecessor.into())
                && status.consumed_command.load(Acquire) == fence.admitted_command_ticket
                && !Self::runtime_fence_changed(status, active, fence)
        });
        if !current {
            candidate.playback.disarm_resident_mute();
        }
    }
}
