//! Correlate a hosted route change with its exact audio publication.
use super::*;
use hex_audio::prepared::AppliedAudioSource;
use hex_engine::playback::{HostedMoveRequest, Playback};
use hex_session::service::{
    ResidentPublicationAcceptance, ResidentPublicationPolicy, ResidentRuntimeLoop,
};

#[derive(Clone, Copy)]
pub(in crate::native_audio) struct MoveRoute {
    request: HostedMoveRequest,
    predecessor: ResidentRuntimeLoop,
    pub(super) applied: AppliedAudioSource,
    context: AudioSourceContext,
    submitted: bool,
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
            submitted: false,
        }
    }
}

impl Plugins {
    pub(in crate::native_audio) fn mark_hosted_move_publication(
        &self,
        context: AudioSourceContext,
        policy: ResidentPublicationPolicy,
        playback: &mut Playback,
    ) -> Result<(), String> {
        let Some(route) = self
            .hosted_move
            .as_ref()
            .filter(|route| route.context == context && !route.submitted)
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
            HostedMoveRequest::Reorder(reorder) => {
                playback.require_hosted_track_effect_reorder(reorder)
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
            .filter(|route| route.context == context && !route.submitted)
            && matches!(result, ResidentPublicationAcceptance::Submitted { .. })
        {
            route.submitted = true;
        }
    }

    pub(in crate::native_audio) fn finish_hosted_move(&mut self, context: AudioSourceContext) {
        if self
            .hosted_move
            .is_some_and(|route| route.context == context)
        {
            // Ordinary edit completion owns the receiver's activation and
            // the donor's retirement after the exact callback receipt.
            self.hosted_move = None;
        }
    }
}
