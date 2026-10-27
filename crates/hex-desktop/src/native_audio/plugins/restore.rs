//! Stopped renewal borrows Root's existing source fence and original return
//! carriers. These scalar stages never stand in for graph/endpoint return or ACK.
use super::*;
use hex_audio::prepared::{
    PublicationControl, PublicationPolicy, StoppedEditFence, StoppedEditFenceState,
};

pub(super) struct Restore {
    context: AudioSourceContext,
    source: hex_audio::prepared::AppliedAudioSource,
    fence: StoppedEditFence,
    stage: Stage,
    result: Option<Result<(), String>>,
    submitted: Option<u64>,
}
enum Stage {
    Fence,
    Return,
    Renew(usize),
    Flush(usize),
    Prepare(usize),
    Ready,
}
impl Restore {
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn observation(
        &self,
        control: &PublicationControl,
    ) -> (&'static str, StoppedEditFenceState) {
        let stage = match self.stage {
            Stage::Fence => "fence",
            Stage::Return => "return",
            Stage::Renew(_) => "renew",
            Stage::Flush(_) => "flush",
            Stage::Prepare(_) => "prepare",
            Stage::Ready => "ready",
        };
        (stage, control.stopped_edit_fence_state(self.fence))
    }
    pub(super) fn finish_gesture_graph(
        &self,
        context: AudioSourceContext,
        source: hex_audio::prepared::AppliedAudioSource,
        control: &mut PublicationControl,
    ) -> bool {
        if self.context != context
            || source.context != context
            || self.source.output_identity != source.output_identity
            || self.submitted != Some(source.audio_revision)
        {
            return false;
        }
        control.release_stopped_edit_fence(self.fence);
        true
    }
    pub(super) fn begin_gesture_rollback(
        &mut self,
        context: AudioSourceContext,
        source: hex_audio::prepared::AppliedAudioSource,
        control: &mut PublicationControl,
    ) -> bool {
        if self.context != context
            || self.source.output_identity != source.output_identity
            || self.submitted != Some(source.audio_revision)
            || source.context != context
        {
            return false;
        }
        if control
            .advance_ordinary_native_epoch_fence(self.fence, self.source, source)
            .is_err()
        {
            return false;
        }
        self.source = source;
        self.submitted = None;
        self.result = None;
        self.stage = Stage::Return;
        true
    }
    // A refused callback did not install the submitted graph. Keep the same
    // fence and original source until the newly loaned endpoints are returned
    // and the previously accepted parameter is flushed back to the instance.
    pub(super) fn begin_refused_gesture_rollback(
        &mut self,
        receipt: hex_session::service::ResidentPublicationReceipt,
        control: &PublicationControl,
    ) -> bool {
        if self.context != receipt.context
            || self.source.output_identity != receipt.output_identity
            || self.submitted != Some(receipt.audio_revision)
            || control.output_identity() != Some(receipt.output_identity)
            || control.stopped_edit_fence_state(self.fence) != StoppedEditFenceState::Ready
        {
            return false;
        }
        self.submitted = None;
        self.result = None;
        self.stage = Stage::Return;
        true
    }
    pub(super) fn ready_for_gesture(&self, context: AudioSourceContext) -> bool {
        self.context == context && matches!(self.result, Some(Ok(()))) && self.submitted.is_none()
    }
    // Before Return the original graph and native source remain in place and
    // no SDK point has been sent. Later errors require their own recovery.
    pub(super) fn gesture_fence_refusal(&self, context: AudioSourceContext) -> Option<&str> {
        if self.context != context
            || !matches!(self.stage, Stage::Fence)
            || self.submitted.is_some()
        {
            return None;
        }
        self.result.as_ref()?.as_ref().err().map(String::as_str)
    }
    pub(super) fn has_original_output(&self, control: &PublicationControl) -> bool {
        control.output_identity() == Some(self.source.output_identity)
            && control.status.recording_source() == Some(self.source)
    }
    pub(super) fn submitted(&self) -> bool {
        self.submitted.is_some()
    }
    pub(super) fn output_closed(&mut self) {
        self.result = Some(Err("stopped vst3 restore output changed".into()));
    }
}
impl Plugins {
    pub fn prepare_restore(
        &mut self,
        output: &mut WorkerPreparedOutput,
        context: AudioSourceContext,
    ) -> Result<Option<bool>, String> {
        if let Some(restore) = self.restore.as_ref() {
            if restore.context != context {
                return Err("stopped vst3 restore source changed".into());
            }
            return restore.result.as_ref().map_or(Ok(Some(false)), |result| {
                result.as_ref().map(|()| Some(true)).map_err(Clone::clone)
            });
        }
        if !self
            .sources
            .iter()
            .any(|source| source.accepted && !source.retiring)
        {
            return Ok(None);
        }
        let control = output.publication.control().ok_or("native output closed")?;
        // Constructor/sampler restoration has its own existing owner. This
        // adapter renews only the ordinary graphs it compiled from its sources.
        if control.status.applied_audio_source().is_some() {
            return Ok(None);
        }
        let source = control
            .status
            .recording_source()
            .ok_or("ordinary vst3 restore source unavailable")?;
        if self.pending.is_some() || source.context != context {
            return Err("stopped vst3 source is not current".into());
        }
        let fence = control
            .request_ordinary_native_epoch_fence(source)
            .map_err(str::to_owned)?;
        #[cfg(all(test, feature = "allocation-probe"))]
        if let Some(observed) = &self.progress
            && let Ok(mut observed) = observed.try_lock()
        {
            observed.stopped_error = None;
        }
        self.restore = Some(Restore {
            context,
            source,
            fence,
            stage: Stage::Fence,
            result: None,
            submitted: None,
        });
        Ok(Some(false))
    }
    pub fn restore_ready(&self) -> bool {
        self.restore
            .as_ref()
            .is_some_and(|restore| restore.result.is_some())
    }
    pub fn cancel_restore(&mut self, control: Option<&mut PublicationControl>) {
        if self
            .restore
            .as_ref()
            .is_some_and(|restore| restore.submitted.is_some())
        {
            return;
        }
        if let Some(restore) = self.restore.take()
            && let Some(control) = control
            && control.output_identity() == Some(restore.source.output_identity)
        {
            control.release_stopped_edit_fence(restore.fence);
        }
    }
    pub fn restore_policy(
        &self,
        context: AudioSourceContext,
    ) -> Result<Option<PublicationPolicy>, String> {
        let Some(restore) = self.restore.as_ref() else {
            return Ok(None);
        };
        if restore.context != context
            || !matches!(restore.result, Some(Ok(())))
            || restore.submitted.is_some()
        {
            return Err("stopped vst3 restoration is not ready for publication".into());
        }
        Ok(Some(PublicationPolicy::InstallUnderNativeEpoch(
            restore.fence,
        )))
    }
    pub fn restore_submitted(&mut self, revision: u64) {
        if let Some(restore) = self.restore.as_mut() {
            restore.submitted = Some(revision);
        }
    }
    pub fn poll_restore(
        &mut self,
        control: &mut PublicationControl,
        context: Option<AudioSourceContext>,
        cancellation: Option<&hex_session::service::RequestCancellation>,
    ) {
        let play_won = self.restore.as_ref().is_some_and(|restore| {
            matches!(restore.stage, Stage::Fence)
                && restore.result.is_none()
                && control.stopped_edit_fence_state(restore.fence) == StoppedEditFenceState::Refused
                && context == Some(restore.context)
                && control.output_identity() == Some(restore.source.output_identity)
                && control.status.recording_source() == Some(restore.source)
                && (control.status.playing.load(Acquire)
                    || control
                        .stopped_native_unsettled_stream_cut(restore.fence)
                        .is_some())
                && control.native_source_control_open()
                && !control.status.device_fault.load(Acquire)
                && !control.status.render_fault.load(Acquire)
                && !cancellation.is_some_and(|request| request.is_cancelled())
                && self.stopped_gesture_point.as_ref().is_some_and(|point| {
                    point.context == restore.context && !point.flushed && point.fence_requested
                })
        });
        if play_won {
            if let Some(restore) = self.restore.take() {
                let unsettled = control.stopped_native_unsettled_stream_cut(restore.fence);
                control.release_stopped_edit_fence(restore.fence);
                if let Some(point) = self.stopped_gesture_point.as_mut() {
                    point.wait_stream_cut = unsettled;
                }
            }
            if let Some(point) = self.stopped_gesture_point.as_mut() {
                point.fence_requested = false;
            }
            return;
        }
        let Some(restore) = self.restore.as_mut() else {
            return;
        };
        if let Some(revision) = restore.submitted {
            if control.output_identity() == Some(restore.source.output_identity)
                && control.status.applied_revision.load(Acquire) == revision
            {
                if self.stopped_gesture_point.is_some() {
                    // Session consumes the exact callback receipt before it
                    // releases this fence or starts cancellation rollback.
                    return;
                }
                control.release_stopped_edit_fence(restore.fence);
                self.restore = None;
            }
            return;
        }
        if restore.result.is_some() {
            return;
        }
        let result: Result<bool, String> = (|| {
            if context != Some(restore.context)
                || control.output_identity() != Some(restore.source.output_identity)
            {
                return Err("stopped vst3 restore source changed".into());
            }
            match restore.stage {
                Stage::Fence => match control.stopped_edit_fence_state(restore.fence) {
                    StoppedEditFenceState::Pending => {}
                    StoppedEditFenceState::Refused => {
                        if control.status.device_fault.load(Acquire)
                            || control.status.render_fault.load(Acquire)
                        {
                            return Err("audio output fault while preparing vst3 parameter".into());
                        }
                        return Err("stop playback before restoring vst3 effects".into());
                    }
                    StoppedEditFenceState::Ready => restore.stage = Stage::Return,
                },
                Stage::Return => match control
                    .return_stopped_ordinary_native_epoch_runtime(restore.source, restore.fence)
                {
                    Ok(()) => restore.stage = Stage::Renew(0),
                    Err("native epoch publication is busy") => {}
                    Err(reason) => return Err(reason.into()),
                },
                Stage::Renew(index) => {
                    if let Some(source) = self.sources.get_mut(index) {
                        if source.accepted && !source.retiring {
                            match control.quiesce_native_source_preparation(
                                source.binding,
                                source.binding.graph,
                            ) {
                                Ok(binding) => {
                                    let previous = source.binding;
                                    source.binding = binding;
                                    source.queued = false;
                                    if let Some(point) = self.stopped_gesture_point.as_mut()
                                        && point.binding == previous
                                    {
                                        point.binding = binding;
                                        if let Some(gesture) = self.gesture.as_mut()
                                            && gesture.binding == previous
                                        {
                                            gesture.binding = binding;
                                        }
                                    }
                                }
                                Err(
                                    "native preparation is busy"
                                    | "native parameter mailbox is busy"
                                    | "native epoch requires parameter input terminal"
                                    | "native epoch requires original graph and endpoint return",
                                ) => return Ok(false),
                                Err(reason) => return Err(reason.into()),
                            }
                        }
                        restore.stage = Stage::Renew(index + 1);
                    } else {
                        restore.stage = if let Some(point) = self.stopped_gesture_point.as_ref() {
                            let index = self
                                .sources
                                .iter()
                                .position(|source| source.binding == point.binding)
                                .ok_or("stopped vst3 gesture source disappeared")?;
                            Stage::Flush(index)
                        } else {
                            Stage::Prepare(0)
                        };
                    }
                }
                Stage::Flush(index) => {
                    let point = self
                        .stopped_gesture_point
                        .as_mut()
                        .ok_or("stopped vst3 gesture disappeared")?;
                    let source = self
                        .sources
                        .get(index)
                        .ok_or("stopped vst3 source disappeared")?;
                    let gesture = self.gesture.ok_or("stopped vst3 gesture disappeared")?;
                    if point.binding != source.binding
                        || point.context != restore.context
                        || point.output != restore.source.output_identity
                        || gesture.binding != point.binding
                    {
                        return Err("stopped vst3 gesture identity changed".into());
                    }
                    if !point.cancel_requested || point.rollback {
                        let value = if point.rollback {
                            gesture.last_value
                        } else {
                            point.value
                        };
                        match control.flush_native_source_parameter_on_control(
                            point.binding,
                            gesture.origin.parameter,
                            value.normalized(),
                            &mut || cancellation.is_some_and(|request| request.is_cancelled()),
                        ) {
                            Ok(Some(true)) => {
                                point.flushed = !point.rollback;
                                point.rollback = false;
                            }
                            Ok(Some(false)) | Ok(None) => {
                                return Err("stopped vst3 parameter flush refused".into());
                            }
                            Err("native preparation is busy") => return Ok(false),
                            Err(reason) => return Err(reason.into()),
                        }
                    }
                    restore.stage = Stage::Prepare(0);
                }
                Stage::Prepare(index) => {
                    let Some(source) = self.sources.get_mut(index) else {
                        restore.stage = Stage::Ready;
                        return Ok(true);
                    };
                    if !source.accepted || source.retiring {
                        restore.stage = Stage::Prepare(index + 1);
                        return Ok(false);
                    }
                    #[cfg(not(all(test, feature = "allocation-probe")))]
                    let presentation = None;
                    #[cfg(all(test, feature = "allocation-probe"))]
                    let presentation = self.presentation;
                    let host = self
                        .host
                        .as_ref()
                        .ok_or("qualified vst3 helper unavailable")?;
                    if advance_source(host, presentation, control, source)?.is_some() {
                        restore.stage = Stage::Prepare(index + 1);
                    }
                }
                Stage::Ready => return Ok(true),
            }
            Ok(false)
        })();
        match result {
            Ok(true) => restore.result = Some(Ok(())),
            Ok(false) => {}
            Err(error) => {
                #[cfg(all(test, feature = "allocation-probe"))]
                if let Some(observed) = &self.progress
                    && let Ok(mut observed) = observed.try_lock()
                {
                    observed.stopped_error = Some(error.clone());
                }
                restore.result = Some(Err(bounded_error(error)));
            }
        }
    }
}
