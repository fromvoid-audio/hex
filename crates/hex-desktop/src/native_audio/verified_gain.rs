//! One coalesced worker metadata overlay and scalar freshness tags. Native
//! source domains, constructor cells, tickets and reclamation keep their owners.
use super::*;
use hex_session::{
    device_gesture::GestureKey,
    service::{DevicePreviewIdentity, VerifiedDeviceParameterSource, VerifiedTrackGainSource},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VerifiedKey {
    context: AudioSourceContext,
    preview: Option<DevicePreviewIdentity>,
}
#[derive(Clone)]
pub(super) enum VerifiedSource {
    Canonical(Arc<CanonicalAudioSourceSnapshot>),
    Gain(Arc<VerifiedTrackGainSource>),
    Device(Arc<VerifiedDeviceParameterSource>),
}
impl VerifiedSource {
    pub(super) fn canonical(&self) -> &CanonicalAudioSourceSnapshot {
        self.canonical_arc().as_ref()
    }
    pub(super) fn canonical_arc(&self) -> &Arc<CanonicalAudioSourceSnapshot> {
        match self {
            Self::Canonical(source) => source,
            Self::Gain(source) => source.canonical(),
            Self::Device(source) => source.canonical(),
        }
    }
    pub(super) fn context(&self) -> AudioSourceContext {
        self.canonical().context()
    }
    pub(super) fn preview_identity(&self) -> Option<DevicePreviewIdentity> {
        match self {
            Self::Canonical(_) => None,
            Self::Gain(source) => source.preview_identity(),
            Self::Device(source) => source.preview_identity(),
        }
    }
    fn terminal_key(&self) -> Option<GestureKey> {
        match self {
            Self::Canonical(_) => None,
            Self::Gain(source) => source.terminal_key(),
            Self::Device(source) => source.terminal_key(),
        }
    }
    fn terminal_sequence(&self) -> Option<u64> {
        match self {
            Self::Canonical(_) => None,
            Self::Gain(source) => source.terminal_sequence(),
            Self::Device(source) => source.terminal_sequence(),
        }
    }
    pub(super) fn key(&self) -> VerifiedKey {
        VerifiedKey {
            context: self.context(),
            preview: self.preview_identity(),
        }
    }
    pub(super) fn candidate_reservation_quote(
        &self,
    ) -> Result<hex_engine::AudioCandidateReservationQuote, hex_engine::AudioStorageError> {
        match self {
            Self::Canonical(source) => source.candidate_reservation_quote(),
            Self::Gain(source) => source.candidate_reservation_quote(),
            Self::Device(source) => source.candidate_reservation_quote(),
        }
    }
    pub(super) fn prepare_candidate_on_control(
        &self,
        catalog: u64,
        destination: &mut hex_engine::AudioCandidatePreparation,
    ) -> Result<(), hex_engine::AudioStorageError> {
        match self {
            Self::Canonical(source) => source.prepare_candidate_on_control(catalog, destination),
            Self::Gain(source) => source.prepare_candidate_on_control(catalog, destination),
            Self::Device(source) => source.prepare_candidate_on_control(catalog, destination),
        }
    }
}
#[derive(Clone, Copy)]
pub(super) struct PreviewWatermark {
    key: GestureKey,
    sequence: u64,
    authoring_revision: u64,
    closed: bool,
}
impl NativeAudioSink {
    pub(super) fn receive_verified_gain(&mut self, source: Arc<VerifiedTrackGainSource>) {
        self.receive_verified_preview(VerifiedSource::Gain(source));
    }
    pub(super) fn receive_verified_device(&mut self, source: Arc<VerifiedDeviceParameterSource>) {
        self.receive_verified_preview(VerifiedSource::Device(source));
    }
    fn receive_verified_preview(&mut self, source: VerifiedSource) {
        let Some(canonical) = self.desired.as_ref() else {
            return;
        };
        // This is the actual source installed by canonical_changed immediately
        // before the worker projection. Equal frontend numbers grant no authority.
        if !Arc::ptr_eq(canonical, source.canonical_arc())
            || canonical.audio_mode() != DocumentAudioMode::VerifiedStreaming
        {
            return;
        }
        if let Some(identity) = source.preview_identity() {
            if self.preview_watermark.is_some_and(|old| {
                identity.key.generation != old.key.generation
                    || identity.key.token < old.key.token
                    || (identity.key == old.key
                        && (old.closed
                            || identity.sequence < old.sequence
                            || identity.canonical_revision < old.authoring_revision))
            }) {
                return;
            }
            if self
                .verified_preview
                .as_ref()
                .and_then(|s| s.preview_identity())
                == Some(identity)
            {
                return;
            }
            self.preview_watermark = Some(PreviewWatermark {
                key: identity.key,
                sequence: identity.sequence,
                authoring_revision: identity.canonical_revision,
                closed: false,
            });
            self.verified_preview = Some(source);
        } else if let Some(key) = source.terminal_key() {
            if self.preview_watermark.is_some_and(|old| {
                key.generation != old.key.generation || key.token < old.key.token
            }) {
                return;
            }
            let previous = self.preview_watermark.filter(|old| old.key == key);
            self.preview_watermark = Some(PreviewWatermark {
                key,
                sequence: previous
                    .map_or(0, |old| old.sequence)
                    .max(source.terminal_sequence().unwrap_or(0)),
                authoring_revision: previous.map_or(canonical.context().editor_revision, |old| {
                    old.authoring_revision
                }),
                closed: true,
            });
            self.verified_preview = None;
        }
    }
}
