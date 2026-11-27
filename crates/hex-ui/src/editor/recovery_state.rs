//! Adapter over [`hex_ui_model::recovery_state`] binding session/engine/audio types.
use hex_engine::AudioSourceContext;
use hex_session::device_gesture::{GestureKey, GestureSnapshot};
use hex_session::service::Request;
pub(super) use hex_ui_model::recovery_state::DeferredEditKind;
use hex_ui_model::recovery_state::SeekOutcome;

#[derive(Clone, Copy, Debug)]
pub(super) struct StrandedGestureRecovery {
    pub(super) key: GestureKey,
    pub(super) reconcilable: Option<GestureSnapshot>,
    pub(super) had_stranded_commit: bool,
    pub(super) is_plugin: bool,
}

pub(super) struct DeferredGraphEdit {
    pub(super) kind: DeferredEditKind,
    pub(super) context: AudioSourceContext,
    pub(super) request: Box<Request>,
    pub(super) stranded_gesture: Option<StrandedGestureRecovery>,
}

#[derive(Default)]
pub(super) struct RecoveryState {
    inner: hex_ui_model::recovery_state::RecoveryState<AudioSourceContext, DeferredGraphEdit>,
}

impl std::ops::Deref for RecoveryState {
    type Target =
        hex_ui_model::recovery_state::RecoveryState<AudioSourceContext, DeferredGraphEdit>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl std::ops::DerefMut for RecoveryState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl RecoveryState {
    pub(super) fn holds_clip_play_start(&self) -> bool {
        self.inner
            .deferred_edit
            .as_ref()
            .is_some_and(|edit| edit.kind == DeferredEditKind::ClipPlayStart)
    }

    pub(super) fn holds_mixer_edit(&self) -> bool {
        self.inner
            .deferred_edit
            .as_ref()
            .is_some_and(|edit| edit.kind == DeferredEditKind::MixerControl)
    }

    pub(super) fn holds_loop_region(&self) -> bool {
        self.inner
            .deferred_edit
            .as_ref()
            .is_some_and(|edit| edit.kind == DeferredEditKind::LoopRegion)
    }

    pub(super) fn holds_stranded_gesture_for(&self, key: GestureKey) -> bool {
        self.inner.deferred_edit.as_ref().is_some_and(|edit| {
            edit.stranded_gesture
                .is_some_and(|stranded| stranded.key == key)
        })
    }

    pub(super) fn observe_receipt(&mut self, receipt: hex_audio::prepared::ActionOutcomeReceipt) {
        if receipt.outcome
            == hex_audio::prepared::ActionOutcome::Rejected(
                hex_engine::PlaybackActionRejection::StreamActionBusy,
            )
            && self.inner.retry_busy_seek(receipt.ticket)
        {
            return;
        }
        let outcome = if receipt.outcome == hex_audio::prepared::ActionOutcome::Applied {
            SeekOutcome::Applied
        } else {
            SeekOutcome::Rejected
        };
        self.inner.observe_receipt(receipt.ticket, outcome);
    }
}
