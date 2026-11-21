//! Canonical route correlation. The original native source and worker stay in Root.
use super::*;
use hex_audio::prepared::AppliedAudioSource;
use hex_engine::playback::{HostedInstrumentMove, Playback};
use hex_session::service::{
    ResidentGraphSubmission, ResidentHostedInstrumentMove, ResidentPublicationAcceptance,
    ResidentPublicationPolicy, ResidentRuntimeLoop,
};

pub(super) struct MoveRoute {
    request: HostedInstrumentMove,
    predecessor: ResidentRuntimeLoop,
    context: AudioSourceContext,
    phase: Phase,
}

enum Phase {
    Preparing,
    Submitted(ResidentGraphSubmission),
    Adopted,
}

impl MoveRoute {
    pub(super) fn prepare(
        context: AudioSourceContext,
        route: ResidentHostedInstrumentMove,
        binding: Binding,
    ) -> Self {
        Self {
            request: HostedInstrumentMove {
                donor: route.donor,
                receiver: route.receiver,
                device: route.device,
                binding,
            },
            predecessor: route.predecessor,
            context,
            phase: Phase::Preparing,
        }
    }
}

impl Plugins {
    pub(super) fn has_adopted_hosted_route(&self, context: AudioSourceContext) -> bool {
        self.hosted_move.as_ref().is_some_and(|route| {
            matches!(route.phase, Phase::Adopted)
                && route.context.project_generation == context.project_generation
                && self.sources.iter().any(|source| {
                    source.binding == route.request.binding && source.accepted && !source.retiring
                })
        })
    }

    pub(in crate::native_audio) fn mark_hosted_move_publication(
        &self,
        context: AudioSourceContext,
        policy: ResidentPublicationPolicy,
        playback: &mut Playback,
    ) -> Result<(), String> {
        let Some(route) = self
            .hosted_move
            .as_ref()
            .filter(|route| route.context == context && matches!(route.phase, Phase::Preparing))
        else {
            return Ok(());
        };
        if !matches!(policy, ResidentPublicationPolicy::DeviceInsert { predecessor, confirmation: None, .. } if predecessor == route.predecessor)
        {
            return Err("hosted instrument move predecessor differs".into());
        }
        playback
            .require_hosted_instrument_move(route.request)
            .map_err(|error| error.0.into())
    }

    pub(in crate::native_audio) fn hosted_move_submitted(
        &mut self,
        context: AudioSourceContext,
        result: &ResidentPublicationAcceptance,
    ) {
        if let Some(route) = self
            .hosted_move
            .as_mut()
            .filter(|route| route.context == context && matches!(route.phase, Phase::Preparing))
            && let ResidentPublicationAcceptance::Submitted {
                output_identity,
                audio_revision,
            } = *result
        {
            route.phase = Phase::Submitted(ResidentGraphSubmission {
                output_identity,
                audio_revision,
                context,
            });
        }
    }

    pub(in crate::native_audio) fn finish_hosted_move(
        &mut self,
        context: AudioSourceContext,
        accepted: bool,
        applied: Option<AppliedAudioSource>,
    ) {
        if !self
            .hosted_move
            .as_ref()
            .is_some_and(|route| route.context == context && !matches!(route.phase, Phase::Adopted))
        {
            return;
        }
        let Some(mut route) = self.hosted_move.take() else {
            return;
        };
        let Phase::Submitted(submission) = route.phase else {
            return;
        };
        if !accepted
            || applied
                != Some(AppliedAudioSource {
                    context: submission.context,
                    output_identity: submission.output_identity,
                    audio_revision: submission.audio_revision,
                })
        {
            return;
        }
        if let Some(source) = self.sources.iter_mut().find(|source| {
            source.binding == route.request.binding
                && source.accepted
                && !source.retiring
                && source.owner == ProcessorChainOwner::Track(route.request.donor.value())
                && source.project_generation == context.project_generation
        }) {
            source.owner = ProcessorChainOwner::Track(route.request.receiver.value());
            route.phase = Phase::Adopted;
            self.hosted_move = Some(route);
        }
    }

    pub(super) fn adopted_hosted_route(
        &self,
        context: AudioSourceContext,
        selected: &[Binding],
        active: Option<AppliedAudioSource>,
    ) -> Result<Option<RetainedInstrumentSource>, String> {
        let Some(route) = self.hosted_move.as_ref().filter(|route| {
            matches!(route.phase, Phase::Adopted)
                && route.context.project_generation == context.project_generation
                && selected.contains(&route.request.binding)
                && self.sources.iter().any(|source| {
                    source.binding == route.request.binding
                        && source.owner
                            == ProcessorChainOwner::Track(route.request.receiver.value())
                        && source.accepted
                        && !source.retiring
                })
        }) else {
            return Ok(None);
        };
        let active = active
            .filter(|applied| applied.context.project_generation == context.project_generation)
            .ok_or("adopted hosted instrument output changed")?;
        let request = route.request;
        RetainedInstrumentSource::for_track_move(
            active,
            request.donor.value(),
            request.receiver.value(),
            request.device.value(),
            request.binding,
        )
        .map(Some)
        .map_err(|error| error.to_string())
    }
}
