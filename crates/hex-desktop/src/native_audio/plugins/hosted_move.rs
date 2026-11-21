//! Correlate a hosted route change with its exact audio publication.
use super::*;
use hex_audio::prepared::AppliedAudioSource;
use hex_engine::playback::{HostedMoveRequest, Playback};
use hex_session::service::{
    ResidentGraphSubmission, ResidentPublicationAcceptance, ResidentPublicationPolicy,
    ResidentRuntimeLoop,
};

#[derive(Clone, Copy)]
pub(in crate::native_audio) struct MoveRoute {
    request: HostedMoveRequest,
    predecessor: ResidentRuntimeLoop,
    pub(super) applied: AppliedAudioSource,
    context: AudioSourceContext,
    submission: Option<ResidentGraphSubmission>,
}

#[derive(Clone, Copy)]
pub(super) struct AdoptedRoute {
    pub(super) request: HostedMoveRequest,
    project_generation: u64,
}

impl MoveRoute {
    pub(super) fn prepare(
        context: AudioSourceContext,
        predecessor: ResidentRuntimeLoop,
        request: HostedMoveRequest,
        applied: AppliedAudioSource,
    ) -> Self {
        Self {
            request,
            predecessor,
            applied,
            context,
            submission: None,
        }
    }

    pub(super) fn binding_for(
        self,
        destination: PluginEffectDestination,
        device: u64,
    ) -> Option<Binding> {
        let HostedMoveRequest::Instrument(request) = self.request else {
            return None;
        };
        (destination.is_instrument()
            && destination.owner() == ProcessorChainOwner::Track(request.receiver.value())
            && device == request.device.value())
        .then_some(request.binding)
    }

    pub(super) fn compiler_alias(self) -> Result<Option<RetainedHostedSource>, String> {
        compiler_alias(self.request, self.applied)
    }
}

impl Plugins {
    pub(super) fn adopted_move(&self, context: AudioSourceContext) -> Option<AdoptedRoute> {
        self.adopted_hosted_move.filter(|route| {
            route.project_generation == context.project_generation
                && self.sources.iter().any(|source| {
                    source.binding == route.request.binding()
                        && source.owner
                            == ProcessorChainOwner::Track(route.request.receiver().value())
                        && source.accepted
                        && !source.retiring
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
            .filter(|route| route.context == context && route.submission.is_none())
        else {
            return Ok(());
        };
        if !matches!(policy, ResidentPublicationPolicy::DeviceInsert { predecessor, confirmation: None, .. } if predecessor == route.predecessor)
        {
            return Err("hosted move predecessor differs".into());
        }
        match route.request {
            HostedMoveRequest::Instrument(movement) => {
                playback.require_hosted_instrument_move(movement)
            }
            HostedMoveRequest::Effect(movement) => {
                playback.require_hosted_track_effect_move(movement)
            }
        }
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
            .filter(|route| route.context == context && route.submission.is_none())
            && let ResidentPublicationAcceptance::Submitted {
                output_identity,
                audio_revision,
            } = *result
        {
            route.submission = Some(ResidentGraphSubmission {
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
        let Some(route) = self.hosted_move.filter(|route| route.context == context) else {
            return;
        };
        self.hosted_move = None;
        if matches!(route.request, HostedMoveRequest::Effect(_)) {
            // The fresh worker was prepared under its final owner. Ordinary
            // edit completion accepts or releases it using the exact receipt.
            return;
        }
        let Some(submission) = route.submission else {
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
            source.binding == route.request.binding()
                && source.accepted
                && !source.retiring
                && source.owner == ProcessorChainOwner::Track(route.request.donor().value())
                && source.project_generation == context.project_generation
        }) {
            source.owner = ProcessorChainOwner::Track(route.request.receiver().value());
            self.adopted_hosted_move = (route.request.receiver() != route.request.physical_owner())
                .then_some(AdoptedRoute {
                    request: route.request,
                    project_generation: context.project_generation,
                });
        }
    }

    pub(super) fn adopted_hosted_route(
        &self,
        context: AudioSourceContext,
        selected: &[Binding],
        active: Option<AppliedAudioSource>,
    ) -> Result<Option<RetainedHostedSource>, String> {
        let Some(route) = self
            .adopted_move(context)
            .filter(|route| selected.contains(&route.request.binding()))
        else {
            return Ok(None);
        };
        let active = active
            .filter(|applied| applied.context.project_generation == context.project_generation)
            .ok_or("adopted hosted move output changed")?;
        compiler_alias(route.request, active)
    }
}

fn compiler_alias(
    request: HostedMoveRequest,
    active: AppliedAudioSource,
) -> Result<Option<RetainedHostedSource>, String> {
    let HostedMoveRequest::Instrument(request) = request else {
        return Ok(None);
    };
    if request.physical_owner == request.receiver {
        return Ok(None);
    }
    RetainedInstrumentSource::for_track_move(
        active,
        request.physical_owner.value(),
        request.receiver.value(),
        request.device.value(),
        request.binding,
    )
    .map(RetainedHostedSource::Instrument)
    .map(Some)
    .map_err(|error| error.to_string())
}
