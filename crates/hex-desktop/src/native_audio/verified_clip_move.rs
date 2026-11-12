//! Bounded source-bound clip intent on the native control worker. No graph is
//! admitted here; callback application remains a separate observation.
use super::*;
use hex_session::service::{
    ClipMovePreviewIdentity, VerifiedClipMoveSource, VerifiedClipMoveUpdate,
};

#[derive(Default)]
pub(super) struct VerifiedClipControl {
    preview: Option<Arc<VerifiedClipMoveSource>>,
    watermark: Option<(ClipMovePreviewIdentity, bool)>,
}
impl VerifiedClipControl {
    pub(super) fn reset(&mut self) {
        self.preview = None;
        self.watermark = None;
    }

    pub(super) fn receive(
        &mut self,
        desired: Option<&Arc<CanonicalAudioSourceSnapshot>>,
        update: VerifiedClipMoveUpdate,
    ) -> bool {
        let source = match &update {
            VerifiedClipMoveUpdate::Preview(preview) => preview.canonical(),
            VerifiedClipMoveUpdate::Released { canonical, .. }
            | VerifiedClipMoveUpdate::Cancelled { canonical, .. } => canonical,
        };
        if desired.is_none_or(|current| !Arc::ptr_eq(current, source))
            || source.audio_mode() != DocumentAudioMode::VerifiedStreaming
        {
            return false;
        }
        match update {
            VerifiedClipMoveUpdate::Preview(preview) => {
                if preview.candidate_reservation_quote().is_err() {
                    return false;
                }
                let identity = preview.identity();
                if self.watermark.is_some_and(|(old, closed)| {
                    identity.context != old.context
                        || identity.gesture < old.gesture
                        || (identity.gesture == old.gesture
                            && (closed || identity.sequence <= old.sequence))
                }) {
                    return false;
                }
                self.watermark = Some((identity, false));
                self.preview = Some(preview);
                true
            }
            VerifiedClipMoveUpdate::Released { identity, .. } => {
                let Some((old, false)) = self.watermark else {
                    return false;
                };
                if identity.context != old.context
                    || identity.gesture != old.gesture
                    || identity.sequence < old.sequence
                    || self.preview.is_none()
                {
                    return false;
                }
                self.watermark = Some((identity, true));
                true
            }
            VerifiedClipMoveUpdate::Cancelled { identity, .. } => {
                let Some(preview) = self.preview.as_ref() else {
                    return false;
                };
                if preview.identity() != identity {
                    return false;
                }
                self.preview = None;
                self.watermark = Some((identity, true));
                true
            }
        }
    }
}
